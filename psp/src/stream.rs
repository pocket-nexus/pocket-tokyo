//! The near level, a cell at a time.
//!
//! A cell's near level is one record of the pack's `NEAR` section: its
//! vertices, its indices and a picture of its ground, 90 KiB or so. The
//! machine keeps the records of the cells before the eye in a fixed set of
//! slots. The frame thread decides which cells those are, once a frame; a
//! thread of lower priority reads them, one at a time, while the frame thread
//! waits for the GE or the display, and writes the shadows into their
//! pictures (`shade`). A cell that is not in memory yet is drawn at the mid
//! level.
//!
//! A slot the eye has left is retired before the frame's draws are chosen and
//! handed on only after the GE has finished the list before: no list in
//! flight reads memory that is being read into.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use psp::sys::*;
use tokyo_pack::{Cell, City, NearCell};
use tokyo_sim::mat;
use tokyo_sim::math::*;

use crate::shade::{self, Ground};
use crate::{mem, store};

pub const SLOTS: usize = 40;
const FREE: u32 = 0;
const WANTED: u32 = 1;
const READY: u32 = 2;
const RETIRED: u32 = 3;
/// A cell is read when the eye comes this near the distance at which it would be drawn, and dropped when it
/// has left that by twice as much.
const AHEAD: f32 = 48.0;

pub struct Slot {
    state: AtomicU32,
    pub cell: u32,
    /// Where the record is in the pack file, and its size.
    at: u32,
    size: u32,
    /// How far the cell is, for the order of reading.
    rank: AtomicU32,
    pub mem: *mut u8,
    /// The palette of the cell's picture as the hour mixes it, and what that mix holds.
    pub mixed: *mut u32,
    pub held: u32,
    /// The picture, for the shadows, and the sweep whose shadows it holds.
    ground: Ground,
    epoch: AtomicU32,
}

const EMPTY: Ground = Ground { data: ptr::null_mut(), width: 0, levels: 0, x0: 0.0, z0: 0.0, side: 0.0 };
static mut TABLE: [Slot; SLOTS] = [const { Slot { state: AtomicU32::new(FREE), cell: u32::MAX, at: 0, size: 0, rank: AtomicU32::new(0), mem: ptr::null_mut(), mixed: ptr::null_mut(), held: u32::MAX, ground: EMPTY, epoch: AtomicU32::new(0) } }; SLOTS];
static mut FD: SceUid = SceUid(-1);
/// Bytes read and reads made since the start, and the time they took.
pub static READ_BYTES: AtomicU32 = AtomicU32::new(0);
pub static READS: AtomicU32 = AtomicU32::new(0);
pub static READ_US: AtomicU32 = AtomicU32::new(0);
/// Pictures the shadows were written into, and the time that took.
pub static SHADED: AtomicU32 = AtomicU32::new(0);
pub static SHADE_US: AtomicU32 = AtomicU32::new(0);

pub struct Streamer {
    records: Vec<NearCell>,
    /// Per cell: its slot, or -1.
    slot_of: Vec<i8>,
    pub slot_bytes: usize,
}

/// Bytes of a slot: the largest record, and no less than a block of the kernel's.
pub fn slot_bytes(records: &[NearCell]) -> usize {
    let largest = records.iter().map(|r| r.size as usize).max().unwrap_or(0);
    ((largest + 63) & !63).max(64 * 1024)
}

unsafe fn shade_slot(s: &Slot) {
    let epoch = shade::EPOCH.load(Ordering::Acquire);
    let t0 = sceKernelGetSystemTimeLow();
    shade::patch(&s.ground);
    s.epoch.store(epoch, Ordering::Release);
    SHADE_US.fetch_add(sceKernelGetSystemTimeLow().wrapping_sub(t0), Ordering::Relaxed);
    SHADED.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn reader(_: usize, _: *mut c_void) -> i32 {
    loop {
        // The nearest cell that is wanted; failing that, the nearest whose shadows are an older sweep's.
        let epoch = shade::EPOCH.load(Ordering::Acquire);
        let nearest = |want: u32, stale: bool| {
            let mut pick: Option<usize> = None;
            for (i, s) in (*ptr::addr_of!(TABLE)).iter().enumerate() {
                if s.state.load(Ordering::Acquire) == want && (!stale || s.epoch.load(Ordering::Relaxed) != epoch) && pick.map_or(true, |p| s.rank.load(Ordering::Relaxed) < TABLE[p].rank.load(Ordering::Relaxed)) {
                    pick = Some(i);
                }
            }
            pick
        };
        if let Some(i) = nearest(WANTED, false) {
            let s = &TABLE[i];
            let t0 = sceKernelGetSystemTimeLow();
            sceIoLseek32(FD, s.at as i32, IoWhence::Set);
            let ok = store::read_exact(FD, s.mem, s.size as usize);
            READ_US.fetch_add(sceKernelGetSystemTimeLow().wrapping_sub(t0), Ordering::Relaxed);
            READ_BYTES.fetch_add(s.size, Ordering::Relaxed);
            READS.fetch_add(1, Ordering::Relaxed);
            if !ok {
                sceKernelDelayThread(200_000);
                continue;
            }
            shade_slot(s);
            // The GE reads memory itself: what the read left in the data cache must be there.
            sceKernelDcacheWritebackRange(s.mem as *const c_void, s.size);
            s.state.store(READY, Ordering::Release);
        } else if let Some(i) = nearest(READY, true) {
            shade_slot(&TABLE[i]);
        } else {
            sceKernelDelayThread(5_000);
        }
    }
}

/// How far a cell is for the slots: its distance, more when it lies behind the eye, where it is not drawn.
fn range(c: &Cell, eye: V3, look: V3) -> f32 {
    let d = mat::box_distance(eye, &c.min, &c.max);
    if d < 140.0 {
        return d;
    }
    let to = v3((c.min[0] + c.max[0]) * 0.5 - eye.x, 0.0, (c.min[2] + c.max[2]) * 0.5 - eye.z);
    let flat = v3(look.x, 0.0, look.z);
    // (looking straight down, everything around is before the eye)
    let cosine = if flat.len2() < 0.05 { 1.0 } else { to.norm_or(flat).dot(flat.norm()) };
    d * if cosine > 0.3 { 1.0 } else { 2.2 }
}

impl Streamer {
    pub unsafe fn start(file: &store::PackFile, records: Vec<NearCell>) -> Result<Streamer, &'static str> {
        let near = file.section(tokyo_pack::NEAR)?.offset;
        let slot_bytes = slot_bytes(&records);
        // The slots and their palettes stay for the rest of the run, at their exact size.
        let mixed = mem::permanent(SLOTS * 1024) as *mut u32;
        if mixed.is_null() {
            return Err("no memory for the cells' slots");
        }
        ptr::write_bytes(mixed, 0, SLOTS * 256);
        for i in 0..SLOTS {
            // (64 KiB or more: a block of the kernel's while it has room, which starts on a 256-byte boundary)
            let m = mem::permanent(slot_bytes);
            if m.is_null() {
                return Err("no memory for the cells' slots");
            }
            TABLE[i].mem = m;
            TABLE[i].mixed = mixed.add(i * 256);
        }
        FD = store::cells().ok_or("the pack did not open a second time")?;
        let id = sceKernelCreateThread(b"tokyo_cells\0".as_ptr(), reader, 36, 16 * 1024, ThreadAttributes::USER, ptr::null_mut());
        if id.0 < 0 {
            store::LAST_CODE = id.0;
            return Err("the cells' thread was not created");
        }
        sceKernelStartThread(id, 0, ptr::null_mut());
        let slot_of = alloc::vec![-1i8; records.len()];
        let records = records
            .into_iter()
            .map(|mut r| {
                r.offset += near;
                r
            })
            .collect();
        Ok(Streamer { records, slot_of, slot_bytes })
    }

    /// Whether a cell's near level is in memory.
    pub fn ready(&self, cell: usize) -> bool {
        let s = self.slot_of[cell];
        s >= 0 && unsafe { TABLE[s as usize].state.load(Ordering::Acquire) == READY }
    }

    /// The slot of a cell that is ready, and its record.
    pub unsafe fn slot(&mut self, cell: usize) -> (&'static mut Slot, &NearCell) {
        (&mut (*ptr::addr_of_mut!(TABLE))[self.slot_of[cell] as usize], &self.records[cell])
    }

    /// Cells in memory, and cells waiting to be read.
    pub fn counts(&self) -> (u32, u32) {
        let (mut ready, mut wanted) = (0, 0);
        for s in unsafe { (*ptr::addr_of!(TABLE)).iter() } {
            match s.state.load(Ordering::Relaxed) {
                READY => ready += 1,
                WANTED => wanted += 1,
                _ => {}
            }
        }
        (ready, wanted)
    }

    /// Before a frame's draws are chosen: the cells the eye has left are no longer ready. `reach`: the distance
    /// within which cells are drawn at the near level.
    pub unsafe fn retire(&mut self, cells: &[Cell], eye: V3, look: V3, reach: f32) {
        for s in (*ptr::addr_of_mut!(TABLE)).iter_mut() {
            let state = s.state.load(Ordering::Acquire);
            if state != READY && state != WANTED {
                continue;
            }
            let d = range(&cells[s.cell as usize], eye, look);
            s.rank.store(d as u32, Ordering::Relaxed);
            // (a cell being read stays until it has arrived)
            if state == READY && d > reach + AHEAD * 2.0 {
                self.slot_of[s.cell as usize] = -1;
                s.state.store(RETIRED, Ordering::Release);
            }
        }
    }

    /// With no list in flight: the retired slots are free, and the nearest cells without a slot take them.
    pub unsafe fn refill(&mut self, city: &City, cells: &[Cell], eye: V3, look: V3, reach: f32) {
        let table = &mut *ptr::addr_of_mut!(TABLE);
        let mut free = 0;
        for s in table.iter_mut() {
            if s.state.load(Ordering::Acquire) == RETIRED {
                s.cell = u32::MAX;
                s.state.store(FREE, Ordering::Release);
            }
            free += (s.state.load(Ordering::Acquire) == FREE) as usize;
        }
        if free == 0 {
            return;
        }
        let mut best = [(f32::MAX, usize::MAX); SLOTS];
        let want = reach + AHEAD;
        for (id, c) in cells.iter().enumerate() {
            if self.slot_of[id] >= 0 || self.records[id].size == 0 || c.min[1] > c.max[1] {
                continue;
            }
            // (most cells are far off: the plain distance first)
            if mat::box_distance(eye, &c.min, &c.max) >= want {
                continue;
            }
            let d = range(c, eye, look);
            if d >= want || d >= best[free - 1].0 {
                continue;
            }
            let mut k = free - 1;
            while k > 0 && best[k - 1].0 > d {
                best[k] = best[k - 1];
                k -= 1;
            }
            best[k] = (d, id);
        }
        let n = city.cells as usize;
        let side = city.block / n as f32;
        let mut next = 0;
        for (i, s) in table.iter_mut().enumerate() {
            if s.state.load(Ordering::Acquire) != FREE {
                continue;
            }
            let (d, id) = best[next];
            if id == usize::MAX {
                break;
            }
            next += 1;
            let r = &self.records[id];
            let (block, cell) = (id / (n * n), id % (n * n));
            let picture: usize = r.parts[..4].iter().map(|p| (*p as usize + 15) & !15).sum();
            s.cell = id as u32;
            s.at = r.offset;
            s.size = r.size;
            s.held = u32::MAX;
            s.ground = Ground {
                data: s.mem.add(picture + 2048),
                width: r.width as u32,
                levels: r.levels as u32,
                x0: city.x0 + (block % city.blocks_x as usize) as f32 * city.block + (cell % n) as f32 * side,
                z0: city.z0 + (block / city.blocks_x as usize) as f32 * city.block + (cell / n) as f32 * side,
                side,
            };
            s.epoch.store(0, Ordering::Relaxed);
            s.rank.store(d as u32, Ordering::Relaxed);
            self.slot_of[id] = i as i8;
            s.state.store(WANTED, Ordering::Release);
        }
    }
}
