//! Shadows on a machine without programs.
//!
//! The pack holds the city as heights, 4 m a cell. For the sun's direction,
//! `tokyo_sim::shadow::sweep` gives each cell the height below which a point
//! over it is in shadow; where that is above the cell's own top, the ground
//! (or the roof) there is in shadow. That is one byte per cell: how far under.
//!
//! The pictures of the ground are 8-bit indices that keep below 128, and the
//! upper half of the palette the GE reads is the lower half as the shadow
//! leaves it. So a texel is put in shadow by setting the top bit of its index,
//! in place, in the picture. A thread of low priority does it in the time the
//! frame thread waits for the GE: it sweeps when the sun has moved, then goes
//! over the blocks' pictures; the cells' thread does the same for the cells it
//! reads (`stream`).

use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use psp::sys::*;
use tokyo_pack::City;
use tokyo_sim::math::V3;

use crate::store;

/// A picture of the ground, where the shadows are written: its largest level (swizzled indices, the smaller
/// levels after it), its side in texels, and the square of the city it shows.
#[derive(Clone, Copy)]
pub struct Ground {
    pub data: *mut u8,
    pub width: u32,
    pub levels: u32,
    pub x0: f32,
    pub z0: f32,
    pub side: f32,
}

struct State {
    heights: *const u16,
    w: usize,
    h: usize,
    x0: f32,
    z0: f32,
    step: f32,
    unit: f32,
    swept: *mut u16,
    mask: *mut u8,
    blocks: *const Ground,
    block_count: usize,
}

static mut STATE: State = State { heights: ptr::null(), w: 0, h: 0, x0: 0.0, z0: 0.0, step: 1.0, unit: 1.0, swept: ptr::null_mut(), mask: ptr::null_mut(), blocks: ptr::null(), block_count: 0 };
/// Towards the sun, as the frame thread last saw it (bits of three floats).
static WANT: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
/// Counts the sweeps: a picture is up to date when it holds the shadows of the last one.
pub static EPOCH: AtomicU32 = AtomicU32::new(0);
pub static SWEEP_MS: AtomicU32 = AtomicU32::new(0);
pub static BLOCKS_MS: AtomicU32 = AtomicU32::new(0);

/// What keeps the thread's memory alive.
pub struct Shade {
    _swept: Vec<u16>,
    _mask: Vec<u8>,
    _blocks: Vec<Ground>,
}

/// Writes the shadows of the last sweep into a picture.
pub unsafe fn patch(g: &Ground) {
    let s = &*ptr::addr_of!(STATE);
    if s.mask.is_null() || EPOCH.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut data = g.data;
    // Per column of the picture: the cell of the grid to its left, and how far to the next, in 256ths.
    let mut cols = [(0u16, 0u16); 512];
    for l in 0..g.levels {
        let w = (g.width >> l) as usize;
        if w < 16 || w > 512 {
            break;
        }
        let scale = g.side / w as f32 / s.step;
        let at = |origin: f32, k: usize, most: usize| -> i32 {
            let f = ((origin + (k as f32 + 0.5) * scale - 0.5) * 256.0) as i32;
            f.clamp(0, (most as i32 - 1) * 256 - 1)
        };
        let (ox, oz) = ((g.x0 - s.x0) / s.step, (g.z0 - s.z0) / s.step);
        for (x, c) in cols[..w].iter_mut().enumerate() {
            let f = at(ox, x, s.w);
            *c = ((f >> 8) as u16, (f & 255) as u16);
        }
        // Per row: the same.
        let mut rows = [(0u16, 0u16); 512];
        for (y, r) in rows[..w].iter_mut().enumerate() {
            let f = at(oz, y, s.h);
            *r = ((f >> 8) as u16, (f & 255) as u16);
        }
        // The picture is stored in blocks of 16 by 8 texels. Most blocks lie whole in the sun or whole in a
        // shadow: the cells under such a block agree, and its bits are set four texels at a time.
        for by in 0..w / 8 {
            let (j0, j1) = (rows[by * 8].0 as usize, rows[by * 8 + 7].0 as usize + 1);
            for bx in 0..w / 16 {
                let block = data.add((by * (w / 16) + bx) * 128);
                let (i0, i1) = (cols[bx * 16].0 as usize, cols[bx * 16 + 15].0 as usize + 1);
                let first = *s.mask.add(j0 * s.w + i0);
                let mut even = first == 0 || first == 255;
                if even {
                    'cells: for j in j0..=j1 {
                        let row = s.mask.add(j * s.w);
                        for i in i0..=i1 {
                            if *row.add(i) != first {
                                even = false;
                                break 'cells;
                            }
                        }
                    }
                }
                if even {
                    let bit = if first == 0 { 0 } else { 0x8080_8080u32 };
                    let words = block as *mut u32;
                    for k in 0..32 {
                        *words.add(k) = (*words.add(k) & 0x7f7f_7f7f) | bit;
                    }
                    continue;
                }
                // On a shadow's edge: texel by texel, between the four cells around it.
                for y in 0..8 {
                    let (j, v) = (rows[by * 8 + y].0 as usize, rows[by * 8 + y].1 as u32);
                    let row0 = s.mask.add(j * s.w);
                    let row1 = row0.add(s.w);
                    for x in 0..16 {
                        let (i, u) = (cols[bx * 16 + x].0 as usize, cols[bx * 16 + x].1 as u32);
                        let (a, b, c, d) = (*row0.add(i) as u32, *row0.add(i + 1) as u32, *row1.add(i) as u32, *row1.add(i + 1) as u32);
                        let under = ((a * (256 - u) + b * u) * (256 - v) + (c * (256 - u) + d * u) * v) >> 16;
                        let p = block.add(y * 16 + x);
                        *p = (*p & 0x7f) | (((under >= 128) as u8) << 7);
                    }
                }
            }
        }
        sceKernelDcacheWritebackRange(data as *const c_void, (w * w) as u32);
        data = data.add(w * w);
    }
}

unsafe extern "C" fn worker(_: usize, _: *mut c_void) -> i32 {
    let s = &*ptr::addr_of!(STATE);
    let mut done = [0.0f32; 3];
    loop {
        let want = [0, 1, 2].map(|k| f32::from_bits(WANT[k].load(Ordering::Relaxed)));
        // (the cosine of one degree: the tip of a 60 m shadow moves a metre)
        let moved = want[0] * done[0] + want[1] * done[1] + want[2] * done[2] < 0.999_85;
        if !moved || want == [0.0; 3] {
            sceKernelDelayThread(100_000);
            continue;
        }
        done = want;
        let t0 = sceKernelGetSystemTimeLow();
        let cells = s.w * s.h;
        tokyo_sim::shadow::sweep(core::slice::from_raw_parts(s.heights, cells), s.w, s.h, core::slice::from_raw_parts_mut(s.swept, cells), s.w, want[0], want[1], want[2], s.step, s.unit);
        // How far under the shadow each cell's top is: 255 at two metres or more.
        let k = (s.unit * 128.0 * 256.0) as u32;
        for i in 0..cells {
            let under = (*s.swept.add(i)).saturating_sub(*s.heights.add(i)) as u32;
            *s.mask.add(i) = ((under * k) >> 8).min(255) as u8;
        }
        let t1 = sceKernelGetSystemTimeLow();
        SWEEP_MS.store(t1.wrapping_sub(t0) / 1000, Ordering::Relaxed);
        EPOCH.fetch_add(1, Ordering::Release);
        for b in 0..s.block_count {
            patch(&*s.blocks.add(b));
        }
        BLOCKS_MS.store(sceKernelGetSystemTimeLow().wrapping_sub(t1) / 1000, Ordering::Relaxed);
    }
}

/// Starts the thread. `heights`: the pack's `HMAP`, which stays where it is; `blocks`: the blocks' pictures.
pub unsafe fn start(heights: &[u16], city: &City, blocks: Vec<Ground>) -> Result<Shade, &'static str> {
    let (w, h) = (city.grid_w as usize, city.grid_h as usize);
    let mut swept = alloc::vec![0u16; w * h];
    let mut mask = alloc::vec![0u8; w * h];
    *ptr::addr_of_mut!(STATE) = State { heights: heights.as_ptr(), w, h, x0: city.grid_x0, z0: city.grid_z0, step: city.grid_step, unit: city.height_step, swept: swept.as_mut_ptr(), mask: mask.as_mut_ptr(), blocks: blocks.as_ptr(), block_count: blocks.len() };
    let id = sceKernelCreateThread(b"tokyo_shade\0".as_ptr(), worker, 44, 16 * 1024, ThreadAttributes::USER, ptr::null_mut());
    if id.0 < 0 {
        store::LAST_CODE = id.0;
        return Err("the shadows' thread was not created");
    }
    sceKernelStartThread(id, 0, ptr::null_mut());
    Ok(Shade { _swept: swept, _mask: mask, _blocks: blocks })
}

/// Tells the thread where the sun is.
pub fn aim(dir: V3) {
    for (slot, v) in WANT.iter().zip([dir.x, dir.y, dir.z]) {
        slot.store(v.to_bits(), Ordering::Relaxed);
    }
}
