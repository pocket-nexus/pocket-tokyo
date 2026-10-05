//! Pocket Tokyo on PSP.
//!
//! The same city as the Vita build, lowered by the city compiler for this
//! machine (`profiles/psp30.json`): 480 × 272 at 30 frames a second, the GE's
//! fixed-function pipeline, 24 MB of memory, one stick.
//!
//! Controls: the stick flies (forward and back, turning left and right),
//! triangle and cross look up and down, L and R go down and up, square flies
//! faster. Left and right on the pad turn the clock; up and down set how fast
//! it runs. START returns to the tour.
//!
//! Development loop over PSPLINK: the pack is read from `host0:/tokyo/`,
//! `control.txt` there steers the run and `status.json` reports it.

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

mod allocator;
mod gfx;
mod shade;
mod store;
mod stream;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::Ordering;

use psp::sys::*;
use tokyo_pack::{self as pack, NearCell};
use tokyo_sim::camera::{btn, Input};
use tokyo_sim::flight::{key, name, Flight};

psp::module!("PocketTokyo", 1, 0);

fn psp_main() {
    psp::enable_home_button();
    unsafe {
        if let Err(e) = run() {
            psp::dprintln!("Could not start: {}", e);
            store::note(&format!("{{\"target\":\"psp\",\"stage\":\"failed\",\"error\":\"{e}\",\"code\":\"{:08x}\"}}", core::ptr::addr_of!(store::LAST_CODE).read() as u32));
            loop {
                sceKernelDelayThread(1_000_000);
            }
        }
    }
}

/// The pad as the flight reads it: the camera's sticks and buttons, and the flight's own keys.
fn read_pad(data: &SceCtrlData) -> (Input, u32) {
    let b = data.buttons;
    // A worn stick rests off centre: nothing inside a quarter of its travel, the rest rescaled.
    let axis = |v: u8| {
        let x = (v as f32 - 127.5) / 127.5;
        let m = if x < 0.0 { -x } else { x };
        if m < 0.24 {
            0.0
        } else {
            (m - 0.24) / 0.76 * if x < 0.0 { -1.0 } else { 1.0 }
        }
    };
    let held = |k: CtrlButtons| b.contains(k);
    let mut buttons = 0;
    for (from, to) in [(CtrlButtons::SQUARE, btn::FAST), (CtrlButtons::RTRIGGER, btn::UP), (CtrlButtons::LTRIGGER, btn::DOWN)] {
        if held(from) {
            buttons |= to;
        }
    }
    let mut keys = 0;
    for (from, to) in [(CtrlButtons::START, key::TOUR), (CtrlButtons::SELECT, key::STATS), (CtrlButtons::RIGHT, key::LATER), (CtrlButtons::LEFT, key::EARLIER), (CtrlButtons::UP, key::FASTER), (CtrlButtons::DOWN, key::SLOWER)] {
        if held(from) {
            keys |= to;
        }
    }
    // One stick: it flies ahead and turns; two buttons look up and down.
    let pitch = (held(CtrlButtons::TRIANGLE) as i32 - held(CtrlButtons::CROSS) as i32) as f32 * 0.7;
    (Input { buttons, lx: 0.0, ly: -axis(data.ly), rx: axis(data.lx), ry: pitch }, keys)
}

/// The Pocket3D title card, drawn into video memory before the GE is set up.
unsafe fn title() {
    // the uncached mirror of video memory: what is written is what the display reads
    let vram = (sceGeEdramGetAddr() as usize | 0x4000_0000) as *mut u8;
    sceDisplaySetMode(DisplayMode::Lcd, 480, 272);
    let mut surface = pocket3d_title::Surface { pixels: core::slice::from_raw_parts_mut(vram, 512 * 272 * 4), width: 480, height: 272, stride: 512, layout: pocket3d_title::Layout::Rgba8 };
    pocket3d_title::play(&mut surface, |_| {
        sceDisplaySetFrameBuf(vram, 512, DisplayPixelFormat::Psm8888, DisplaySetBufSync::NextFrame);
        sceDisplayWaitVblankStart();
    });
}

/// Rolling frame statistics over the last `N` frames.
struct Timing {
    ms: [f32; Timing::N],
    at: usize,
    late: u32,
    frames: u32,
}

impl Timing {
    const N: usize = 60;
    fn push(&mut self, ms: f32, late: bool) {
        self.ms[self.at] = ms;
        self.at = (self.at + 1) % Timing::N;
        self.frames += 1;
        self.late += late as u32;
    }
    fn avg(&self) -> f32 {
        self.ms.iter().sum::<f32>() / Timing::N as f32
    }
    fn worst(&self) -> f32 {
        self.ms.iter().fold(0.0f32, |a, b| if *b > a { *b } else { a })
    }
}

unsafe fn run() -> Result<(), &'static str> {
    scePowerSetClockFrequency(333, 333, 166);
    // Test runs: `boot.txt` on the share holds control words for the start, plus `title=0` (no title card),
    // `shot=N` (write frame N to `shot.raw`) and `exit=N` (leave at frame N).
    let mut boot_buf = [0u8; 512];
    let boot: &str = store::boot_text(&mut boot_buf).unwrap_or("");
    if !boot.split_ascii_whitespace().any(|w| w == "title=0") {
        title();
    }
    psp::dprintln!("Pocket Tokyo\n");
    let t_load = sceKernelGetSystemTimeLow();
    let free_at_start = sceKernelTotalFreeMemSize();
    let file = store::PackFile::open()?;
    let mut stage = |name: &str| {
        psp::dprintln!("  {}", name);
        if file.host {
            store::note(&format!("{{\"target\":\"psp\",\"stage\":\"loading\",\"step\":\"{name}\"}}"));
        }
    };
    let mut gfx = gfx::Gfx::load(&file, &mut stage)?;
    let tour: Vec<[f32; 6]> = file.records(pack::TOUR)?;
    // The area's name, from the pack's own description of itself.
    let meta: Vec<u8> = file.records(pack::META)?;
    let place: String = core::str::from_utf8(&meta).ok().and_then(|m| m.split("\"name\":\"").nth(1)).and_then(|m| m.split('"').next()).unwrap_or("Tokyo").into();
    drop(meta);
    let near: Vec<NearCell> = file.records(pack::NCEL)?;
    let mut streamer = stream::Streamer::start(&file, near)?;
    // From here the screen belongs to the GE; messages go to the computer only.
    if file.host {
        store::note("{\"target\":\"psp\",\"stage\":\"loading\",\"step\":\"start\"}");
    }
    store::start(&file)?;
    let mut flight = Flight::new(gfx.city.view, gfx.city.hour, tour, 42_000, (600.0, 1600.0), 2);
    flight.clearance = 30.0;
    flight.control(boot);
    // (a frame number no run reaches when the word is absent)
    let (shot_at, exit_at) = (flight.number(name(b"shot"), 4.0e9) as u32, flight.number(name(b"exit"), 4.0e9) as u32);
    // Which of the two frame buffers the list in flight draws into.
    let mut drawing = 0usize;
    let load_ms = sceKernelGetSystemTimeLow().wrapping_sub(t_load) / 1000;
    let free_after = sceKernelTotalFreeMemSize();

    sceCtrlSetSamplingCycle(0);
    sceCtrlSetSamplingMode(CtrlMode::Analog);
    let mut data: SceCtrlData = core::mem::zeroed();
    let mut status = String::with_capacity(2048);
    let mut line = String::with_capacity(160);
    let (mut drawn_last, mut draws_last) = (0u32, 0u32);
    let mut timing = Timing { ms: [33.3; Timing::N], at: 0, late: 0, frames: 0 };
    let (mut gpu_ms, mut select_ms, mut list_ms, mut ge_ms) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut last_swap = sceKernelGetSystemTimeLow();
    let mut last_vcount = sceDisplayGetVcount();
    let mut ticks = flight.pace;
    let mut frame = 0u32;
    let ms = |from: u32| sceKernelGetSystemTimeLow().wrapping_sub(from) as f32 / 1000.0;

    loop {
        // -------------------------------------------------------------- input and the flight
        sceCtrlPeekBufferPositive(&mut data, 1);
        store::control(|text| flight.control(text));
        gfx.option = flight.number(name(b"option"), 0.0) as u32;
        let (inp, keys) = read_pad(&data);
        flight.step(&inp, keys, ticks as f32 / 60.0, |x, z| gfx.height(x, z));
        // The frame's draws are chosen while the GE draws the frame before.
        gfx.choose(&flight, &mut streamer);

        // -------------------------------------------------------------- the previous frame leaves the GE
        let t1 = sceKernelGetSystemTimeLow();
        sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
        gpu_ms = gpu_ms * 0.9 + ms(t1) * 0.1;
        if frame == shot_at.wrapping_add(1) {
            // The frame just drawn, out of video memory first: host I/O cannot take a video memory address.
            let vram = sceGeEdramGetAddr().add(drawing * gfx::FB_BYTES);
            let mut pixels = alloc::vec![0u8; 480 * 272 * 2];
            for y in 0..272 {
                core::ptr::copy_nonoverlapping(vram.add(y * 512 * 2), pixels.as_mut_ptr().add(y * 480 * 2), 480 * 2);
            }
            store::write_shot(&pixels);
        }
        if frame >= exit_at {
            sceKernelDelayThread(300_000);
            store::note(&status);
            sceKernelExitGame();
        }
        // Present on a display refresh, `pace` of them after the last frame: 30 frames a second.
        let pace = flight.pace;
        while sceDisplayGetVcount().wrapping_sub(last_vcount) < pace {
            sceDisplayWaitVblankStart();
        }
        sceGuSwapBuffers();
        drawing ^= 1;
        let vcount = sceDisplayGetVcount();
        let refreshes = vcount.wrapping_sub(last_vcount);
        // A late frame catches up, by one refresh at most.
        ticks = refreshes.clamp(1, pace + 1);
        last_vcount = vcount;
        let now = sceKernelGetSystemTimeLow();
        timing.push(now.wrapping_sub(last_swap) as f32 / 1000.0, refreshes > pace);
        last_swap = now;

        // -------------------------------------------------------------- this frame's list
        let t2 = sceKernelGetSystemTimeLow();
        // What the screen says: the place and the hour; with SELECT, the frame's counts.
        line.clear();
        let _ = write!(line, "{}  {:02}:{:02}{}", place, flight.hour as u32, ((flight.hour - (flight.hour as u32) as f32) * 60.0) as u32, if flight.tour_on { "  TOUR" } else { "" });
        gfx.text(12, 246, 2, 0xf0ff_ffff, &line);
        if flight.stats {
            let s = gfx.stats;
            line.clear();
            let _ = write!(line, "{:.1} FPS  {:.1} MS  LATE {}  GE {:.1}  {} TRIS  {} DRAWS  {}/{}/{}", 1000.0 / timing.avg().max(0.1), timing.avg(), timing.late, gpu_ms, drawn_last, draws_last, s.places[0], s.places[1], s.places[2]);
            gfx.text(8, 8, 1, 0xffff_ffff, &line);
        } else if flight.seconds < 10.0 {
            gfx.text(12, 232, 1, 0xc0ff_ffff, "STICK: FLY   TRIANGLE CROSS: LOOK   L R: DOWN, UP   SQUARE: FAST   PAD: THE CLOCK   START: TOUR");
        }
        gfx.draw(&flight, &mut streamer);
        let s = gfx.stats;
        // `profile=1`: wait for the GE here, to time a frame's list from its start to its end.
        if flight.number(name(b"profile"), 0.0) != 0.0 {
            sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            ge_ms = ge_ms * 0.9 + ms(t2) * 0.1;
        }
        select_ms = select_ms * 0.9 + s.select as f32 * 0.0001;
        list_ms = list_ms * 0.9 + s.list as f32 * 0.0001;
        // A tour nobody touches is still being watched: the console's idle timers start again.
        if frame % 120 == 0 {
            scePowerTick(PowerTick::All);
        }
        let drawn = s.tris.iter().sum::<u32>();
        (drawn_last, draws_last) = (drawn, s.draws);
        flight.drew(drawn);
        frame += 1;

        if file.host && frame % 15 == 0 {
            status.clear();
            let (eye, look, _) = flight.eye();
            let (ready, wanted) = streamer.counts();
            let _ = write!(
                status,
                "{{\"target\":\"psp\",\"stage\":\"running\",\"frames\":{},\"frameMs\":{:.2},\"worstMs\":{:.2},\"late\":{},\"tris\":[{},{},{},{}],\"level\":[{},{},{}],\"drawn\":{},\"draws\":{},\"places\":[{},{},{}],\"turned\":{},\"binds\":{},\"mixes\":{},\"gpuMs\":{:.2},\"geMs\":{:.2},\"cpuMs\":{{\"select\":{:.2},\"list\":{:.2}}},\"words\":{},\"reach\":[{:.0},{:.0}],\"governor\":{{\"on\":{},\"budget\":{},\"scale\":{:.3}}},\"clock\":{{\"hour\":{:.3},\"rate\":{:.3}}},\"tour\":{{\"on\":{},\"at\":{:.1},\"seconds\":{:.1}}},\"eye\":[{:.1},{:.1},{:.1}],\"look\":[{:.3},{:.3},{:.3}],\"memory\":{{\"freeAtStart\":{},\"freeAfterLoad\":{},\"free\":{},\"resident\":{},\"slot\":{},\"small\":{},\"large\":{}}},\"cells\":{{\"ready\":{},\"wanted\":{},\"reads\":{},\"readKb\":{},\"readMs\":{},\"shaded\":{},\"shadeMs\":{}}},\"shadows\":{{\"sweeps\":{},\"sweepMs\":{},\"blocksMs\":{}}},\"loadMs\":{},\"pack\":{}}}",
                timing.frames,
                timing.avg(),
                timing.worst(),
                timing.late,
                s.tris[0],
                s.tris[1],
                s.tris[2],
                s.tris[3],
                s.level[0],
                s.level[1],
                s.level[2],
                drawn,
                s.draws,
                s.places[0],
                s.places[1],
                s.places[2],
                s.turned,
                s.binds,
                s.mixes,
                gpu_ms,
                ge_ms,
                select_ms,
                list_ms,
                s.words,
                flight.show.near,
                flight.show.mid,
                flight.governor.on,
                flight.governor.budget,
                flight.governor.scale,
                flight.hour,
                flight.rate,
                flight.tour_on,
                flight.tour_at,
                flight.tour.seconds(),
                eye.x,
                eye.y,
                eye.z,
                look.x,
                look.y,
                look.z,
                free_at_start,
                free_after,
                sceKernelTotalFreeMemSize(),
                gfx.resident_bytes,
                streamer.slot_bytes,
                allocator::arena_used(),
                core::ptr::addr_of!(allocator::LARGE_BYTES).read(),
                ready,
                wanted,
                stream::READS.load(Ordering::Relaxed),
                stream::READ_BYTES.load(Ordering::Relaxed) / 1024,
                stream::READ_US.load(Ordering::Relaxed) / 1000,
                stream::SHADED.load(Ordering::Relaxed),
                stream::SHADE_US.load(Ordering::Relaxed) / 1000,
                shade::EPOCH.load(Ordering::Relaxed),
                shade::SWEEP_MS.load(Ordering::Relaxed),
                shade::BLOCKS_MS.load(Ordering::Relaxed),
                load_ms,
                file.bytes
            );
            store::publish(&status);
        }
    }
}
