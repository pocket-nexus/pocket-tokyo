//! The shadows: for the light's direction, the height below which each cell
//! of the city's height grid is in shadow (`tokyo_sim::shadow`), as a texture
//! every scene program reads. A thread of its own sweeps the grid whenever the
//! light has moved, and the render thread sends a finished sweep to the GPU a
//! strip a frame.
//!
//! The texture is half floats, which this GPU filters, with two smaller
//! levels under it: seen from far away the grid is many texels to a pixel,
//! and a fetch from the largest level alone would miss the texture cache. A
//! sweep goes into the texture no queued frame reads; a tile-based driver
//! copies a texture that a frame still reads before it writes into it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokyo_sim::math::V3;

use crate::gl;

const IDLE: u32 = 0;
const WORKING: u32 = 1;
const READY: u32 = 2;
/// Levels of the texture: the grid, a half and a quarter.
const LEVELS: usize = 3;
/// Texels the render thread sends in a frame.
const STRIP: usize = 2048 * 96;

struct Shared {
    heights: Vec<u16>,
    w: usize,
    h: usize,
    step: f32,
    unit: f32,
    /// The light to sweep for.
    job: Mutex<Option<V3>>,
    /// The last sweep as half floats, level after level.
    out: Mutex<Vec<u16>>,
    state: AtomicU32,
    /// Hundredths of a millisecond the last sweep took.
    took: AtomicU32,
}

/// A value between 0 and 1 as the bits of a half float.
fn half(x: f32) -> u16 {
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127;
    if x <= 0.0 || exponent < -14 {
        0
    } else if exponent >= 0 {
        0x3c00
    } else {
        (((exponent + 15) as u32) << 10 | ((bits >> 13) & 0x3ff)) as u16
    }
}

/// Where each level starts in the sweep's output, its width and its height.
fn levels(w: usize, h: usize) -> [(usize, usize, usize); LEVELS] {
    let mut out = [(0, 0, 0); LEVELS];
    let mut at = 0;
    for (k, level) in out.iter_mut().enumerate() {
        let (lw, lh) = (w >> k, h >> k);
        *level = (at, lw, lh);
        at += lw * lh;
    }
    out
}

impl Shared {
    fn sweep(&self, scratch: &mut [Vec<u16>; LEVELS], table: &[u16], light: V3) {
        let t = Instant::now();
        let (w, h) = (self.w, self.h);
        tokyo_sim::shadow::sweep(&self.heights, w, h, &mut scratch[0], w, light.x, light.y, light.z, self.step, self.unit);
        // The smaller levels: the mean of four.
        for k in 1..LEVELS {
            let (from, to) = scratch.split_at_mut(k);
            let (from, to) = (&from[k - 1], &mut to[0]);
            let (fw, tw, th) = (w >> (k - 1), w >> k, h >> k);
            for y in 0..th {
                for x in 0..tw {
                    let o = y * 2 * fw + x * 2;
                    to[y * tw + x] = ((from[o] as u32 + from[o + 1] as u32 + from[o + fw] as u32 + from[o + fw + 1] as u32) / 4) as u16;
                }
            }
        }
        let mut out = self.out.lock().unwrap();
        let mut at = 0;
        for level in scratch.iter() {
            for (to, v) in out[at..at + level.len()].iter_mut().zip(level) {
                *to = table[*v as usize];
            }
            at += level.len();
        }
        self.took.store((t.elapsed().as_secs_f32() * 100_000.0) as u32, Ordering::Relaxed);
    }
}

pub struct Shadows {
    textures: [u32; 2],
    /// One texel of no shadow, for measurements.
    blank: u32,
    front: usize,
    shared: Arc<Shared>,
    /// The sweep being sent to the back texture: the level and the row it has reached.
    sending: Option<(usize, usize)>,
    /// Frames until the texture that was shown last may be written again: a queued frame may still read it.
    cool: u32,
    /// The light the newest sweep was asked for.
    asked: V3,
    pub sweeps: u32,
    /// Milliseconds the render thread last spent sending a strip.
    pub send_ms: f32,
}

impl Shadows {
    /// # Safety
    /// A current context. `w` and `h` are multiples of 4.
    pub unsafe fn new(heights: Vec<u16>, w: usize, h: usize, step: f32, unit: f32, light: V3) -> Result<Shadows, String> {
        let table: Vec<u16> = (0..65536u32).map(|q| half(q as f32 / 65535.0)).collect();
        let total: usize = levels(w, h).iter().map(|l| l.1 * l.2).sum();
        let shared = Arc::new(Shared { heights, w, h, step, unit, job: Mutex::new(None), out: Mutex::new(vec![0u16; total]), state: AtomicU32::new(IDLE), took: AtomicU32::new(0) });
        let mut scratch: [Vec<u16>; LEVELS] = core::array::from_fn(|k| vec![0u16; (w >> k) * (h >> k)]);
        // The first sweep before the first frame, into both textures.
        shared.sweep(&mut scratch, &table, light);
        let mut textures = [0u32; 2];
        gl::glGenTextures(2, textures.as_mut_ptr());
        {
            let out = shared.out.lock().unwrap();
            for &id in &textures {
                gl::bind(1, id);
                gl::glPixelStorei(gl::UNPACK_ALIGNMENT, 2);
                for (k, (at, lw, lh)) in levels(w, h).into_iter().enumerate() {
                    gl::glTexImage2D(gl::TEXTURE_2D, k as i32, gl::R16F as i32, lw as i32, lh as i32, 0, gl::RED, gl::HALF_FLOAT, out[at..].as_ptr().cast());
                }
                gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAX_LEVEL, LEVELS as i32 - 1);
                gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR_MIPMAP_NEAREST as i32);
                gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
                gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
                gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
            }
        }
        let mut blank = 0;
        gl::glGenTextures(1, &mut blank);
        gl::bind(1, blank);
        gl::glTexImage2D(gl::TEXTURE_2D, 0, gl::R16F as i32, 1, 1, 0, gl::RED, gl::HALF_FLOAT, [0u16; 2].as_ptr().cast());
        gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
        gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
        if gl::glGetError() != 0 {
            return Err("the shadows' texture could not be made".into());
        }
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("tokyo-shadows".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                // Below every thread of a frame: above 60 °C the system leaves the phone two cores, and a
                // sweep is a quarter of a second of one.
                extern "C" {
                    fn setpriority(which: i32, who: u32, priority: i32) -> i32;
                }
                unsafe { setpriority(0, 0, 19) };
                loop {
                    let job = worker.job.lock().unwrap().take();
                    match job {
                        Some(light) => {
                            worker.sweep(&mut scratch, &table, light);
                            worker.state.store(READY, Ordering::Release);
                        }
                        None => std::thread::sleep(Duration::from_millis(4)),
                    }
                }
            })
            .map_err(|e| format!("shadow thread: {e}"))?;
        Ok(Shadows { textures, blank, front: 0, shared, sending: None, cool: 0, asked: light, sweeps: 1, send_ms: 0.0 })
    }

    /// Once a frame: sends a strip of a finished sweep, shows it when all of it is there, and asks for a new
    /// one when the light has turned.
    ///
    /// # Safety
    /// A current context.
    pub unsafe fn update(&mut self, light: V3) {
        match self.shared.state.load(Ordering::Acquire) {
            READY if self.cool == 0 => {
                let t = Instant::now();
                let (mut level, mut row) = self.sending.unwrap_or((0, 0));
                let table = levels(self.shared.w, self.shared.h);
                let out = self.shared.out.lock().unwrap();
                gl::bind(1, self.textures[1 - self.front]);
                gl::glPixelStorei(gl::UNPACK_ALIGNMENT, 2);
                let mut left = STRIP;
                while left > 0 && level < LEVELS {
                    let (at, lw, lh) = table[level];
                    let rows = (left / lw).max(1).min(lh - row);
                    gl::glTexSubImage2D(gl::TEXTURE_2D, level as i32, 0, row as i32, lw as i32, rows as i32, gl::RED, gl::HALF_FLOAT, out[at + row * lw..].as_ptr().cast());
                    left = left.saturating_sub(rows * lw);
                    row += rows;
                    if row >= lh {
                        level += 1;
                        row = 0;
                    }
                }
                self.send_ms = t.elapsed().as_secs_f32() * 1000.0;
                if level >= LEVELS {
                    self.sending = None;
                    self.front = 1 - self.front;
                    self.cool = 3;
                    self.sweeps += 1;
                    self.shared.state.store(IDLE, Ordering::Release);
                } else {
                    self.sending = Some((level, row));
                }
            }
            IDLE if self.cool == 0 => {
                // (a tenth of a degree: at the edge of a shadow a kilometre long, under two metres)
                if self.asked.dot(light) < 0.999_998_5 {
                    self.asked = light;
                    *self.shared.job.lock().unwrap() = Some(light);
                    self.shared.state.store(WORKING, Ordering::Release);
                }
            }
            _ => {}
        }
        self.cool = self.cool.saturating_sub(1);
    }

    /// The texture to draw with this frame.
    pub fn texture(&self) -> u32 {
        self.textures[self.front]
    }

    pub fn none(&self) -> u32 {
        self.blank
    }

    /// Milliseconds the last sweep took.
    pub fn sweep_ms(&self) -> f32 {
        self.shared.took.load(Ordering::Relaxed) as f32 / 100.0
    }
}
