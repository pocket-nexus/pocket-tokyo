//! C interface of the 3DS host (`../src/core.h` declares the same functions),
//! and of the iPod touch's (`ipod/core` builds this source for
//! `armv7-apple-ios`; what differs there is under `target_vendor = "apple"`).
//!
//! The host owns the GPU, the pad and storage; it hands the pack's tables in
//! once and then asks, each frame, where the eye is, what the light is and
//! what to draw. The slots that hold the cells' near levels are decided here
//! and filled by a thread of the host's.

#![no_std]

extern crate alloc;

#[path = "alloc.rs"]
mod allocator;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_char;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering};

use tokyo_pack::{Batch, Block, Cell, City, Landmark, NearCell, Region, KINDS, SECTORS};
use tokyo_sim::camera::Input;
use tokyo_sim::flight::Flight;
use tokyo_sim::mat;
use tokyo_sim::math::*;
use tokyo_sim::sky;
use tokyo_sim::view::{self, Item};

/// Day of the year the sun follows (early October).
const DAY: f32 = 277.0;
/// What the night leaves of the ground's own colour and of a wall's (`NIGHT_GROUND`, `NIGHT_WALL` in the
/// city compiler).
const NIGHT_GROUND: [f32; 3] = [0.13, 0.15, 0.22];
const NIGHT_WALL: [f32; 3] = [0.16, 0.18, 0.26];

pub const SLOTS: usize = 40;
const FREE: u32 = 0;
const WANTED: u32 = 1;
const READY: u32 = 2;
const RETIRED: u32 = 3;
/// A cell is read when the eye comes this near the distance at which it would be drawn, and dropped when it
/// has left that by twice as much.
const AHEAD: f32 = 48.0;

/// The screen's width over its height, and what the status calls the machine.
#[cfg(not(target_vendor = "apple"))]
const SCREEN: (f32, &str) = (400.0 / 240.0, "3ds");
#[cfg(target_vendor = "apple")]
const SCREEN: (f32, &str) = (480.0 / 320.0, "ipod");

/// A slot, as the host's reading thread sees it: it reads `size` bytes at `offset` of the pack for a slot that
/// is `WANTED` and sets it `READY`.
#[repr(C)]
pub struct Slot {
    state: AtomicU32,
    cell: u32,
    offset: u32,
    size: u32,
    rank: AtomicU32,
}

static mut TABLE: [Slot; SLOTS] = [const { Slot { state: AtomicU32::new(FREE), cell: u32::MAX, offset: 0, size: 0, rank: AtomicU32::new(0) } }; SLOTS];

#[repr(C)]
pub struct Pack {
    city: *const City,
    regions: *const Region,
    region_count: u32,
    blocks: *const Block,
    block_count: u32,
    cells: *const Cell,
    cell_count: u32,
    batches: *const Batch,
    batch_count: u32,
    spans: *const u32,
    span_count: u32,
    tour: *const [f32; 6],
    tour_count: u32,
    heights: *const u16,
    near: *const NearCell,
    /// Where `NEAR` starts in the pack file.
    near_offset: u32,
    meta: *const u8,
    meta_len: u32,
    landmarks: *const Landmark,
    landmark_count: u32,
}

#[repr(C)]
pub struct Pad {
    /// `camera::btn` bits, then `flight::key` bits.
    buttons: u32,
    keys: u32,
    lx: f32,
    ly: f32,
    rx: f32,
    ry: f32,
}

/// The frame as the host draws it.
#[repr(C)]
#[derive(Default)]
pub struct View {
    eye: [f32; 3],
    look: [f32; 3],
    fov: f32,
    hour: f32,
    night: f32,
    haze: [f32; 3],
    /// Light on what looks up, halved (the combiner doubles): the day's, mixed with what the night leaves.
    top: [f32; 3],
    /// The same on a wall that looks to each sector, and last on painted faces that look up or down.
    lights: [[f32; 3]; SECTORS + 1],
    /// What a shadow leaves of the light on the ground.
    shade: f32,
    /// Towards the sun, for the shadows; all zero when no shadow is to be drawn anew.
    sun: [f32; 3],
    near: f32,
    mid: f32,
    tour_on: u32,
    stats: u32,
    pace: u32,
    option: u32,
}

#[repr(C)]
pub struct Perf {
    frame: f32,
    worst: f32,
    late: u32,
    frames: u32,
    cpu: f32,
    gpu: f32,
    draws: u32,
    tris: [u32; KINDS],
}

#[repr(C)]
pub struct SkyVertex {
    pos: [f32; 3],
    color: [u8; 4],
}

struct App {
    flight: Flight,
    city: City,
    regions: &'static [Region],
    blocks: &'static [Block],
    cells: &'static [Cell],
    batches: &'static [Batch],
    spans: &'static [u32],
    heights: &'static [u16],
    near: &'static [NearCell],
    near_offset: u32,
    landmarks: &'static [Landmark],
    lists: [Vec<Item>; KINDS],
    slot_of: Vec<i8>,
    counts: view::Counts,
    place: String,
    light: sky::Light,
}

static mut APP: Option<App> = None;

fn app() -> &'static mut App {
    // The host calls from its frame thread, after `tk_init` succeeded.
    unsafe { (*core::ptr::addr_of_mut!(APP)).as_mut().unwrap_unchecked() }
}

fn height(a: &App, x: f32, z: f32) -> f32 {
    let c = &a.city;
    let i = (((x - c.grid_x0) / c.grid_step) as i32).clamp(0, c.grid_w as i32 - 1) as usize;
    let j = (((z - c.grid_z0) / c.grid_step) as i32).clamp(0, c.grid_h as i32 - 1) as usize;
    c.y0 + a.heights[j * c.grid_w as usize + i] as f32 * c.height_step
}

/// Sizes the host's structures must have; a mismatch with `core.h` fails at start.
#[no_mangle]
pub extern "C" fn tk_sizes(out: *mut [u32; 8]) {
    use core::mem::size_of;
    unsafe {
        *out = [size_of::<City>() as u32, size_of::<Batch>() as u32, size_of::<NearCell>() as u32, size_of::<Item>() as u32, size_of::<View>() as u32, size_of::<Slot>() as u32, SLOTS as u32, sky::DOME_VERTS as u32];
    }
}

/// # Safety
/// The pack's tables stay where they are for the life of the program.
#[no_mangle]
pub unsafe extern "C" fn tk_init(p: *const Pack, budget: u32) -> *const c_char {
    let p = &*p;
    let city = core::ptr::read_unaligned(p.city);
    if city.region_blocks != 1 || city.flags & tokyo_pack::flag::STREAMED == 0 || city.cells > 8 {
        return c"the pack is not a handheld pack".as_ptr();
    }
    let tour: Vec<[f32; 6]> = core::slice::from_raw_parts(p.tour, p.tour_count as usize).to_vec();
    let batches = core::slice::from_raw_parts(p.batches, p.batch_count as usize);
    let meta = core::slice::from_raw_parts(p.meta, p.meta_len as usize);
    let place: String = core::str::from_utf8(meta).ok().and_then(|m| m.split("\"name\":\"").nth(1)).and_then(|m| m.split('"').next()).unwrap_or("Tokyo").into();
    let mut flight = Flight::new(city.view, city.hour, tour, budget, (600.0, 1600.0), 2);
    flight.clearance = 30.0;
    *core::ptr::addr_of_mut!(APP) = Some(App {
        flight,
        city,
        regions: core::slice::from_raw_parts(p.regions, p.region_count as usize),
        blocks: core::slice::from_raw_parts(p.blocks, p.block_count as usize),
        cells: core::slice::from_raw_parts(p.cells, p.cell_count as usize),
        batches,
        spans: core::slice::from_raw_parts(p.spans, p.span_count as usize),
        heights: core::slice::from_raw_parts(p.heights, (city.grid_w * city.grid_h) as usize),
        near: core::slice::from_raw_parts(p.near, p.cell_count as usize),
        near_offset: p.near_offset,
        landmarks: core::slice::from_raw_parts(p.landmarks, p.landmark_count as usize),
        lists: core::array::from_fn(|_| Vec::with_capacity(512)),
        slot_of: alloc::vec![-1i8; p.cell_count as usize],
        counts: view::Counts { places: [0; 3], turned: 0, mid_from: f32::MAX },
        place,
        light: sky::light(city.hour, DAY),
    });
    core::ptr::null()
}

#[no_mangle]
pub unsafe extern "C" fn tk_control(text: *const u8, len: u32) {
    if let Ok(t) = core::str::from_utf8(core::slice::from_raw_parts(text, len as usize)) {
        app().flight.control(t);
    }
}

#[no_mangle]
pub unsafe extern "C" fn tk_step(pad: *const Pad, ticks: u32) {
    let a = app();
    let p = &*pad;
    let inp = Input { buttons: p.buttons, lx: p.lx, ly: p.ly, rx: p.rx, ry: p.ry };
    let heights = a.heights;
    let c = a.city;
    a.flight.step(&inp, p.keys, ticks as f32 / 60.0, |x, z| {
        let i = (((x - c.grid_x0) / c.grid_step) as i32).clamp(0, c.grid_w as i32 - 1) as usize;
        let j = (((z - c.grid_z0) / c.grid_step) as i32).clamp(0, c.grid_h as i32 - 1) as usize;
        c.y0 + heights[j * c.grid_w as usize + i] as f32 * c.height_step
    });
    a.light = sky::light(a.flight.hour, DAY);
}

#[no_mangle]
pub unsafe extern "C" fn tk_view(out: *mut View) {
    let a = app();
    let (eye, look, fov) = a.flight.eye();
    let l = &a.light;
    let t = l.night;
    let day = |k: usize| -> [f32; 3] {
        let (facing, up) = if k < SECTORS { (max(view::sector_normal(k).dot(l.dir), 0.0), 0.5) } else { (max(l.dir.y, 0.0), 1.0) };
        [0, 1, 2].map(|c| l.sun[c] * facing + lerp(l.ground[c], l.sky[c], up))
    };
    let top = day(SECTORS);
    let mix = |d: [f32; 3], n: [f32; 3]| [0, 1, 2].map(|c| lerp(d[c], n[c], t) * 0.5);
    // What stands in shadow keeps the sky's light, of what the sun and the sky give together.
    let lit = (top[0] + top[1] + top[2]) / 3.0;
    let shade = lerp(clamp((l.sky[0] + l.sky[1] + l.sky[2]) / 3.0 / max(lit, 0.01), 0.0, 1.0), 1.0, t);
    *out = View {
        eye: [eye.x, eye.y, eye.z],
        look: [look.x, look.y, look.z],
        fov,
        hour: a.flight.hour,
        night: t,
        haze: l.horizon,
        top: mix(top, NIGHT_GROUND),
        lights: core::array::from_fn(|k| mix(day(k), NIGHT_WALL)),
        shade,
        sun: if t < 0.97 { [l.dir.x, l.dir.y, l.dir.z] } else { [0.0; 3] },
        near: a.flight.show.near,
        mid: a.flight.show.mid,
        tour_on: a.flight.tour_on as u32,
        stats: a.flight.stats as u32,
        pace: a.flight.pace,
        option: a.flight.number(tokyo_sim::flight::name(b"option"), 0.0) as u32,
    };
}

/// The sky's dome for this hour: `sky::DOME_VERTS` vertices around the eye.
#[no_mangle]
pub unsafe extern "C" fn tk_sky(out: *mut SkyVertex, radius: f32) {
    let l = &app().light;
    for i in 0..sky::DOME_VERTS {
        let d = sky::dome_dir(i);
        let c = sky::dome_color(l, d);
        *out.add(i) = SkyVertex { pos: [d.x * radius, d.y * radius, d.z * radius], color: [(saturate(c[0]) * 255.0) as u8, (saturate(c[1]) * 255.0) as u8, (saturate(c[2]) * 255.0) as u8, 255] };
    }
}

#[no_mangle]
pub unsafe extern "C" fn tk_sky_indices(out: *mut u16) -> u32 {
    sky::dome_indices(core::slice::from_raw_parts_mut(out, sky::DOME_INDICES));
    sky::DOME_INDICES as u32
}

/// How far a cell is for the slots: its distance, more when it lies behind the eye, where it is not drawn.
fn range(c: &Cell, eye: V3, look: V3) -> f32 {
    let d = mat::box_distance(eye, &c.min, &c.max);
    if d < 140.0 {
        return d;
    }
    let to = v3((c.min[0] + c.max[0]) * 0.5 - eye.x, 0.0, (c.min[2] + c.max[2]) * 0.5 - eye.z);
    let flat = v3(look.x, 0.0, look.z);
    let cosine = if flat.len2() < 0.05 { 1.0 } else { to.norm_or(flat).dot(flat.norm()) };
    d * if cosine > 0.3 { 1.0 } else { 2.2 }
}

/// The slots, for the host's reading thread.
#[no_mangle]
pub extern "C" fn tk_slots() -> *mut Slot {
    core::ptr::addr_of_mut!(TABLE) as *mut Slot
}

/// Chooses the frame's draws. Cells the eye has left stop being ready first; their slots are handed on by
/// `tk_refill`, when the GPU has finished the frame before. Returns the lists (one per kind of batch) and
/// their lengths.
#[no_mangle]
pub unsafe extern "C" fn tk_choose(lists: *mut *const Item, lengths: *mut u32) {
    let a = app();
    let (eye, look, fov) = a.flight.eye();
    let table = &mut *core::ptr::addr_of_mut!(TABLE);
    for s in table.iter_mut() {
        let state = s.state.load(Ordering::Acquire);
        if state != READY && state != WANTED {
            continue;
        }
        let d = range(&a.cells[s.cell as usize], eye, look);
        s.rank.store(d as u32, Ordering::Relaxed);
        if state == READY && d > a.flight.show.near + AHEAD * 2.0 {
            a.slot_of[s.cell as usize] = -1;
            s.state.store(RETIRED, Ordering::Release);
        }
    }
    let vp = mat::mul(&mat::perspective(fov, SCREEN.0, 3.0, 12000.0), &mat::view(eye, look, 0.0));
    let planes = mat::planes(&vp);
    let tables = view::Tables { city: &a.city, regions: a.regions, blocks: a.blocks, cells: a.cells, batches: a.batches, spans: a.spans, landmarks: a.landmarks };
    let slot_of = &a.slot_of;
    let ready = |cell: usize| {
        let s = slot_of[cell];
        s >= 0 && table[s as usize].state.load(Ordering::Acquire) == READY
    };
    a.counts = view::select(&tables, &planes, eye, &a.flight.show, &ready, &mut a.lists);
    for k in 0..KINDS {
        *lists.add(k) = a.lists[k].as_ptr();
        *lengths.add(k) = a.lists[k].len() as u32;
    }
}

/// The slot that holds a cell's near level.
#[no_mangle]
pub extern "C" fn tk_slot_of(cell: u32) -> i32 {
    app().slot_of[cell as usize] as i32
}

/// With no frame in flight: the retired slots are free, and the nearest cells without a slot take them.
#[no_mangle]
pub unsafe extern "C" fn tk_refill() {
    let a = app();
    let (eye, look, _) = a.flight.eye();
    let table = &mut *core::ptr::addr_of_mut!(TABLE);
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
    let want = a.flight.show.near + AHEAD;
    for (id, c) in a.cells.iter().enumerate() {
        if a.slot_of[id] >= 0 || a.near[id].size == 0 || c.min[1] > c.max[1] || mat::box_distance(eye, &c.min, &c.max) >= want {
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
        s.cell = id as u32;
        s.offset = a.near_offset + a.near[id].offset;
        s.size = a.near[id].size;
        s.rank.store(d as u32, Ordering::Relaxed);
        a.slot_of[id] = i as i8;
        s.state.store(WANTED, Ordering::Release);
    }
}

/// After a frame that drew `tris` triangles.
#[no_mangle]
pub extern "C" fn tk_drew(tris: u32) {
    app().flight.drew(tris);
}

/// Where a texel of a square texture is stored. The PICA200: 8-texel squares in rows from the last, each in
/// Morton order.
#[cfg(not(target_vendor = "apple"))]
fn tiled(x: usize, y: usize, side: usize) -> usize {
    let fy = side - 1 - y;
    let mut m = 0;
    for b in 0..3 {
        m |= ((x >> b) & 1) << (2 * b);
        m |= ((fy >> b) & 1) << (2 * b + 1);
    }
    ((fy / 8) * (side / 8) + x / 8) * 64 + m
}

/// OpenGL ES: rows, the first row first.
#[cfg(target_vendor = "apple")]
fn tiled(x: usize, y: usize, side: usize) -> usize {
    y * side + x
}

/// The shadows of a direction of the sun, as a texture of 8-bit light over the grid of heights: 255 in the
/// sun, `shade` of it where the ground stands two metres or more under a shadow. `swept` is scratch for the
/// grid; `texture` is `side` texels square, stored as the machine's GPU reads it (`tiled`). May run on another
/// thread than the frame's.
///
/// # Safety
/// After `tk_init`; `swept` holds the grid, `texture` `side * side` bytes.
#[no_mangle]
pub unsafe extern "C" fn tk_shadows(sun: *const f32, shade: f32, swept: *mut u16, texture: *mut u8, side: u32) {
    let a = (*core::ptr::addr_of!(APP)).as_ref().unwrap_unchecked();
    let c = &a.city;
    let (w, h, side) = (c.grid_w as usize, c.grid_h as usize, side as usize);
    let out = core::slice::from_raw_parts_mut(swept, w * h);
    let dir = [*sun, *sun.add(1), *sun.add(2)];
    tokyo_sim::shadow::sweep(a.heights, w, h, out, w, dir[0], dir[1], dir[2], c.grid_step, c.height_step);
    let k = (c.height_step * 128.0 * 256.0) as u32;
    let dark = (1.0 - clamp(shade, 0.0, 1.0)) * 255.0;
    for j in 0..h.min(side) {
        for i in 0..w.min(side) {
            let under = (((out[j * w + i].saturating_sub(a.heights[j * w + i])) as u32 * k) >> 8).min(255);
            *texture.add(tiled(i, j, side)) = 255 - ((under as f32 * dark) as u32 >> 8) as u8;
        }
    }
}

/// The place's name and the hour, for the lower screen.
#[no_mangle]
pub unsafe extern "C" fn tk_title(out: *mut u8, cap: u32) -> u32 {
    let a = app();
    let mut s = String::with_capacity(96);
    let _ = write!(s, "{}  {:02}:{:02}{}", a.place, a.flight.hour as u32, ((a.flight.hour - (a.flight.hour as u32) as f32) * 60.0) as u32, if a.flight.tour_on { "  TOUR" } else { "" });
    let n = s.len().min(cap as usize - 1);
    core::ptr::copy_nonoverlapping(s.as_ptr(), out, n);
    *out.add(n) = 0;
    n as u32
}

/// The run as a JSON object, for the development host. `extra`: members the host adds.
#[no_mangle]
pub unsafe extern "C" fn tk_status(out: *mut u8, cap: u32, perf: *const Perf, extra: *const u8, extra_len: u32) -> u32 {
    let a = app();
    let p = &*perf;
    let f = &a.flight;
    let (eye, look, _) = f.eye();
    let table = &*core::ptr::addr_of!(TABLE);
    let ready = table.iter().filter(|s| s.state.load(Ordering::Relaxed) == READY).count();
    let wanted = table.iter().filter(|s| s.state.load(Ordering::Relaxed) == WANTED).count();
    let mut s = String::with_capacity(1536);
    let _ = write!(
        s,
        "{{\"target\":\"{}\",\"stage\":\"running\",\"frames\":{},\"frameMs\":{:.2},\"worstMs\":{:.2},\"late\":{},\"cpuMs\":{:.2},\"gpuMs\":{:.2},\"tris\":[{},{},{},{}],\"drawn\":{},\"draws\":{},\"places\":[{},{},{}],\"turned\":{},\"reach\":[{:.0},{:.0}],\"governor\":{{\"on\":{},\"budget\":{},\"scale\":{:.3}}},\"clock\":{{\"hour\":{:.3},\"rate\":{:.3},\"night\":{:.2}}},\"tour\":{{\"on\":{},\"at\":{:.1},\"seconds\":{:.1}}},\"eye\":[{:.1},{:.1},{:.1}],\"look\":[{:.3},{:.3},{:.3}],\"cells\":{{\"ready\":{},\"wanted\":{}}}",
        SCREEN.1,
        p.frames,
        p.frame,
        p.worst,
        p.late,
        p.cpu,
        p.gpu,
        p.tris[0],
        p.tris[1],
        p.tris[2],
        p.tris[3],
        p.tris.iter().sum::<u32>(),
        p.draws,
        a.counts.places[0],
        a.counts.places[1],
        a.counts.places[2],
        a.counts.turned,
        f.show.near,
        f.show.mid,
        f.governor.on,
        f.governor.budget,
        f.governor.scale,
        f.hour,
        f.rate,
        a.light.night,
        f.tour_on,
        f.tour_at,
        f.tour.seconds(),
        eye.x,
        eye.y,
        eye.z,
        look.x,
        look.y,
        look.z,
        ready,
        wanted
    );
    if extra_len > 0 {
        if let Ok(e) = core::str::from_utf8(core::slice::from_raw_parts(extra, extra_len as usize)) {
            s.push(',');
            s.push_str(e);
        }
    }
    s.push('}');
    let n = s.len().min(cap as usize - 1);
    core::ptr::copy_nonoverlapping(s.as_ptr(), out, n);
    *out.add(n) = 0;
    n as u32
}

/// A frame of the Pocket3D title card for a host with no frame buffer a CPU writes: `tick`'s frame as RGBA
/// rows into `pixels`. Returns 0 when the card is over, 2 when the frame is the one drawn at tick `shown`
/// (nothing is written), 1 when it drew.
///
/// # Safety
/// `pixels` has room for `width * height * 4` bytes.
#[cfg(target_vendor = "apple")]
#[no_mangle]
pub unsafe extern "C" fn tk_card(pixels: *mut u8, width: u32, height: u32, tick: u32, shown: u32) -> u32 {
    use pocket3d_title::{draw, level, Layout, Surface, TICKS};
    if tick >= TICKS {
        return 0;
    }
    if shown < TICKS && level(tick) == level(shown) {
        return 2;
    }
    let mut surface = Surface { pixels: core::slice::from_raw_parts_mut(pixels, (width * height * 4) as usize), width, height, stride: width, layout: Layout::Rgba8 };
    draw(&mut surface, tick) as u32
}

/// Whether the tour carries the eye, and the top of what stands under it (for the host's own displays).
#[no_mangle]
pub extern "C" fn tk_ground(x: f32, z: f32) -> f32 {
    height(app(), x, z)
}
