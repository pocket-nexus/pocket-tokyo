//! The shadows: for the light's direction, the height below which each cell
//! of the city's height grid is in shadow (`tokyo_sim::shadow`), as a texture
//! every scene program reads. A thread of its own sweeps the grid whenever the
//! light has moved; the frame that finds a sweep finished starts showing it.

use pocket_vita_gxm::mem::Arena;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokyo_sim::math::V3;
use vita2d_sys as g;

const IDLE: u32 = 0;
const WORKING: u32 = 1;
const READY: u32 = 2;

struct Target(*mut u16);
unsafe impl Send for Target {}
unsafe impl Sync for Target {}

struct Shared {
    heights: Vec<u16>,
    w: usize,
    h: usize,
    step: f32,
    unit: f32,
    targets: [Target; 2],
    /// The light to sweep for and the texture to write.
    job: Mutex<Option<(V3, usize)>>,
    state: AtomicU32,
    /// Hundredths of a millisecond the last sweep took.
    took: AtomicU32,
}

impl Shared {
    /// Sweeps into ordinary memory, where the sweep's own reads are fast, then copies to the texture.
    unsafe fn sweep(&self, scratch: &mut [u16], light: V3, target: usize) {
        let t = Instant::now();
        tokyo_sim::shadow::sweep(&self.heights, self.w, self.h, scratch, self.w, light.x, light.y, light.z, self.step, self.unit);
        core::ptr::copy_nonoverlapping(scratch.as_ptr(), self.targets[target].0, self.w * self.h);
        self.took.store((t.elapsed().as_secs_f32() * 100_000.0) as u32, Ordering::Relaxed);
    }
}

pub struct Shadows {
    textures: [g::SceGxmTexture; 2],
    front: usize,
    shared: Arc<Shared>,
    /// Frames until the texture that was shown last may be written again: the GPU may still be reading it.
    cool: u32,
    /// The light the newest sweep was asked for.
    asked: V3,
    pub sweeps: u32,
}

impl Shadows {
    /// # Safety
    /// GXM is initialized; `vram` outlives the shadows. `w` must be a multiple of 8.
    pub unsafe fn new(vram: &mut Arena, heights: Vec<u16>, w: usize, h: usize, step: f32, unit: f32, light: V3) -> Result<Shadows, String> {
        let mut textures: [g::SceGxmTexture; 2] = core::mem::zeroed();
        let mut targets = [core::ptr::null_mut::<u16>(); 2];
        for k in 0..2 {
            targets[k] = vram.alloc(w * h * 2, 512)?.cast();
            let r = g::sceGxmTextureInitLinear(&mut textures[k], targets[k].cast(), g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U16_RRRR, w as u32, h as u32, 1);
            if r < 0 {
                return Err(format!("shadow texture: sceGxmTextureInitLinear 0x{:08x}", r as u32));
            }
            g::sceGxmTextureSetMinFilter(&mut textures[k], g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
            g::sceGxmTextureSetMagFilter(&mut textures[k], g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
            g::sceGxmTextureSetUAddrMode(&mut textures[k], g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
            g::sceGxmTextureSetVAddrMode(&mut textures[k], g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
        }
        let shared = Arc::new(Shared { heights, w, h, step, unit, targets: [Target(targets[0]), Target(targets[1])], job: Mutex::new(None), state: AtomicU32::new(IDLE), took: AtomicU32::new(0) });
        // The first sweep before the first frame.
        let mut scratch = vec![0u16; w * h];
        shared.sweep(&mut scratch, light, 0);
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("tokyo-shadows".into())
            .stack_size(128 * 1024)
            .spawn(move || loop {
                let job = worker.job.lock().unwrap().take();
                match job {
                    Some((light, target)) => {
                        unsafe { worker.sweep(&mut scratch, light, target) };
                        worker.state.store(READY, Ordering::Release);
                    }
                    None => std::thread::sleep(Duration::from_millis(4)),
                }
            })
            .map_err(|e| format!("shadow thread: {e}"))?;
        Ok(Shadows { textures, front: 0, shared, cool: 0, asked: light, sweeps: 1 })
    }

    /// Once a frame: shows a finished sweep, and asks for a new one when the light has turned.
    pub fn update(&mut self, light: V3) {
        match self.shared.state.load(Ordering::Acquire) {
            READY => {
                self.front = 1 - self.front;
                self.cool = 3;
                self.sweeps += 1;
                self.shared.state.store(IDLE, Ordering::Release);
            }
            IDLE if self.cool == 0 => {
                // (a tenth of a degree: at the edge of a shadow a kilometre long, under two metres)
                if self.asked.dot(light) < 0.999_998_5 {
                    self.asked = light;
                    *self.shared.job.lock().unwrap() = Some((light, 1 - self.front));
                    self.shared.state.store(WORKING, Ordering::Release);
                }
            }
            _ => {}
        }
        self.cool = self.cool.saturating_sub(1);
    }

    /// The texture to draw with this frame.
    pub fn texture(&self) -> &g::SceGxmTexture {
        &self.textures[self.front]
    }

    /// Milliseconds the last sweep took.
    pub fn sweep_ms(&self) -> f32 {
        self.shared.took.load(Ordering::Relaxed) as f32 / 100.0
    }
}
