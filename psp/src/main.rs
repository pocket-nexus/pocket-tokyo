//! Pocket Tokyo on PSP.
//!
//! The same city as the Vita build, lowered by the city compiler for this
//! machine (`profiles/psp30.json`): 480 × 272 at 30 frames a second, the GE's
//! fixed-function pipeline, 24 MB of memory, one stick.
//!
//! Controls in flight: the stick flies (forward and back, turning left and
//! right), triangle and cross look up and down, L and R go down and up,
//! square flies faster, left and right on the pad turn the clock, SELECT
//! hands the eye to the tour and takes it back. Everything else is the
//! interface's (`interface`): a PocketJS guest shared with the other devices,
//! drawn over the city, which START brings up as a menu. Without it (its
//! files are missing, or a 24 MB machine has no room for it beside the city)
//! nothing is drawn over the city and START does what SELECT does.
//!
//! Development loop over PSPLINK: the pack and the interface's bundle are
//! read from `host0:/tokyo/`, `control.txt` there steers the run and
//! `status.json` reports it.

#![no_std]
#![no_main]

extern crate alloc;

mod gfx;
mod interface;
mod mem;
mod shade;
mod store;
mod stream;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Write;
use core::sync::atomic::Ordering;

use pocketjs_psp::{arena, host};
use psp::sys::*;
use tokyo_interface::{channel, pad, parse_f32, Mode, Pad, Session};
use tokyo_pack::{self as pack, City, NearCell};
use tokyo_sim::flight::{name, Flight};

psp::module!("PocketTokyo", 1, 0);

fn psp_main() {
    psp::enable_home_button();
    unsafe {
        // This thread has the EBOOT's directory as its working directory, and no other thread does:
        // it opens what lies there, and once the frame thread runs it serves storage below it.
        store::locate();
        // The frame thread, with this thread's attributes and four times its stack: the interface's
        // guest compiles its bundle and mounts its screens on the thread that runs it, and that
        // takes more than 256 KB.
        let id = sceKernelCreateThread(b"tokyo_frame\0".as_ptr(), frame_thread, 32, 1024 * 1024, ThreadAttributes::USER | ThreadAttributes::VFPU, core::ptr::null_mut());
        if id.0 < 0 {
            psp::dprintln!("Could not start: the frame thread was not created ({:08x})", id.0 as u32);
            loop {
                sceKernelDelayThread(1_000_000);
            }
        }
        sceKernelStartThread(id, 0, core::ptr::null_mut());
        sceKernelChangeThreadPriority(SceUid(sceKernelGetThreadId()), 40);
        store::serve()
    }
}

unsafe extern "C" fn frame_thread(_: usize, _: *mut c_void) -> i32 {
    start();
    0
}

unsafe fn start() {
    // PSPLINK can start a thread with floating-point exceptions on; the interface's layout computes with NaN.
    host::reset_fpu_status();
    let mut ui = interface::Ui::none("");
    let mut on_screen = false;
    if let Err(e) = run(&mut ui, &mut on_screen) {
        store::note(&format!("{{\"target\":\"psp\",\"stage\":\"failed\",\"error\":\"{e}\",\"code\":\"{:08x}\"}}", core::ptr::addr_of!(store::LAST_CODE).read() as u32));
        if !on_screen {
            psp::dprintln!("Could not start: {}", e);
        }
        let state = &mut channel().state;
        state.mode = Mode::Error;
        state.message.clear();
        state.message.push_str(e);
        let mut frames = 0u32;
        // No flight to pace the guest by: it takes every turn.
        let waking = Session::new();
        loop {
            if on_screen && ui.up() {
                ui.turn(interface::TURN, 0, interface::STICK_CENTER, &waking);
                gfx::interlude(&ui);
                frames += 1;
                // A test run (`boot.txt` names a frame to leave at) ends here, with this screen as its frame.
                let mut buf = [0u8; 512];
                if frames == 30 && store::boot_text(&mut buf).is_some_and(|text| text.contains("exit=")) {
                    store::write_shot(&gfx::pixels(gfx::DRAW_BUFFER ^ 1));
                    sceKernelExitGame();
                }
            } else {
                sceKernelDelayThread(1_000_000);
            }
        }
    }
}

/// The pad as the flow reads it: the camera's sticks and buttons, the flight's keys, and the button
/// that is the menu's.
fn read_pad(data: &SceCtrlData) -> Pad {
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
    for (from, to) in [
        (CtrlButtons::SQUARE, pad::FAST),
        (CtrlButtons::RTRIGGER, pad::UP),
        (CtrlButtons::LTRIGGER, pad::DOWN),
        (CtrlButtons::SELECT, pad::TOUR),
        (CtrlButtons::RIGHT, pad::LATER),
        (CtrlButtons::LEFT, pad::EARLIER),
        (CtrlButtons::START, pad::MENU),
    ] {
        if held(from) {
            buttons |= to;
        }
    }
    // One stick: it flies ahead and turns; two buttons look up and down.
    let pitch = (held(CtrlButtons::TRIANGLE) as i32 - held(CtrlButtons::CROSS) as i32) as f32 * 0.7;
    Pad { buttons, lx: 0.0, ly: -axis(data.ly), rx: axis(data.lx), ry: pitch }
}

/// The words of a control text that are the flow's and the interface's: `mode=title|flight|menu`
/// sets the flow outright, `ui=tour|fly|menu|resume|title` asks what the interface's lists ask,
/// `press=<mask>` presses buttons on the interface as a thumb would (PocketJS's button bits, the
/// pad's own), and `rest=<turns>` leaves the pad alone that many of its turns before the next press.
unsafe fn remote(text: &str, ui: &mut interface::Ui, session: &mut Session) {
    for word in text.split_ascii_whitespace() {
        match word.split_once('=') {
            Some(("press", mask)) => {
                if let Some(mask) = parse_f32(mask) {
                    ui.press(mask as u32);
                }
            }
            Some(("rest", turns)) => {
                if let Some(turns) = parse_f32(turns) {
                    ui.rest(turns as u32);
                }
            }
            Some(("mode", name)) => {
                if let Some(mode @ (Mode::Title | Mode::Flight | Mode::Menu)) = Mode::parse(name) {
                    session.mode = mode;
                }
            }
            Some(("ui", what)) => channel().receive(match (what, session.mode == Mode::Title) {
                ("tour", true) => r#"{"type":"start","tour":true}"#,
                ("fly", true) => r#"{"type":"start","tour":false}"#,
                ("tour", false) => r#"{"type":"tour","on":true}"#,
                ("fly", false) => r#"{"type":"tour","on":false}"#,
                ("menu", _) => r#"{"type":"menu","on":true}"#,
                ("resume", _) => r#"{"type":"menu","on":false}"#,
                ("title", _) => r#"{"type":"title"}"#,
                _ => continue,
            }),
            _ => {}
        }
    }
}

/// Triangles a frame of the flight may draw: a list costs the GE 1.5 ms and 0.63 ms per 1 000.
const BUDGET: u32 = 42_000;
/// Triangles fewer while the title is up, and while the menu is: what their pixels cost the GE.
const TITLE_LESS: u32 = 3_000;
const MENU_LESS: u32 = 11_000;

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

unsafe fn run(ui: &mut interface::Ui, on_screen: &mut bool) -> Result<(), &'static str> {
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
    let city: City = *file.records::<City>(pack::CITY)?.first().ok_or("city section")?;
    let near: Vec<NearCell> = file.records(pack::NCEL)?;

    // The interface starts first when memory has room for it beside the city: the arena's tail and
    // what the kernel still has, against the pack's resident sections, the slots of the cells' near
    // levels, the shadows' two grids, what the city works in, and what the interface takes. A
    // PSP-1000's 24 MB hold the city alone; the pad then keeps the flow itself.
    let room = arena::stats().tail_free_bytes + mem::kernel_room();
    let city_bytes = file.resident_bytes() + stream::SLOTS * stream::slot_bytes(&near) + city.grid_w as usize * city.grid_h as usize * 3 + 1536 * 1024;
    let arena_before_ui = arena::stats().bump_bytes;
    if room < city_bytes + interface::RESERVE {
        *ui = interface::Ui::none("no memory for the interface beside the city");
        store::leave_beside(store::SCRIPT);
        store::leave_beside(store::PAK);
    } else {
        psp::dprintln!("  Starting the interface");
        *ui = interface::Ui::boot(store::read_beside(store::SCRIPT, &[0]), store::keep_beside(store::PAK));
        ui.bytes = arena::stats().bump_bytes - arena_before_ui;
    }
    if ui.up() {
        // From here the screen belongs to the GE, and the loading steps to the interface.
        gfx::init();
        *on_screen = true;
    }
    // While the city loads the guest takes every turn it is offered.
    let waking = Session::new();
    let mut stage = |name: &str| {
        if ui.up() {
            let state = &mut channel().state;
            state.message.clear();
            state.message.push_str(name);
            ui.turn(interface::TURN, 0, interface::STICK_CENTER, &waking);
            gfx::interlude(ui);
        } else {
            psp::dprintln!("  {}", name);
        }
        if file.host {
            store::note(&format!("{{\"target\":\"psp\",\"stage\":\"loading\",\"step\":\"{name}\"}}"));
        }
    };
    stage("Reading the city");
    let mut gfx = gfx::Gfx::load(&file, city, &mut stage)?;
    let tour: Vec<[f32; 6]> = file.records(pack::TOUR)?;
    stage("Starting");
    let mut streamer = stream::Streamer::start(&file, near)?;
    store::start(&file);
    if !*on_screen {
        // No interface: the debug text stood until here.
        gfx::init();
        *on_screen = true;
    }
    // What a flight alone may draw, and what the governor was last told.
    let (mut budget, mut applied) = (BUDGET, BUDGET);
    let mut flight = Flight::new(gfx.city.view, gfx.city.hour, tour, budget, (600.0, 1600.0), 2);
    flight.clearance = 30.0;
    let mut session = Session::new();
    // The numbers in flight ten times a second: each refresh is a turn of the guest.
    session.numbers_every = 3;
    {
        // What the interface asked to have kept in an earlier run.
        let mut buf = [0u8; store::KEPT_MAX];
        if let Some(text) = store::kept(&mut buf) {
            let state = &mut channel().state;
            state.prefs.clear();
            state.prefs.push_str(text);
        }
    }
    flight.control(boot);
    remote(boot, ui, &mut session);
    // (a frame number no run reaches when the word is absent)
    let (shot_at, exit_at) = (flight.number(name(b"shot"), 4.0e9) as u32, flight.number(name(b"exit"), 4.0e9) as u32);
    let load_ms = sceKernelGetSystemTimeLow().wrapping_sub(t_load) / 1000;
    let free_after = sceKernelTotalFreeMemSize();
    let arena_after_load = arena::stats().bump_bytes;

    sceCtrlSetSamplingCycle(0);
    sceCtrlSetSamplingMode(CtrlMode::Analog);
    let mut data: SceCtrlData = core::mem::zeroed();
    let mut status = String::with_capacity(3072);
    // What the interface asked to have kept, until the storage thread has taken it.
    let mut prefs: Option<String> = None;
    let (mut drawn_last, mut draws_last) = (0u32, 0u32);
    let mut timing = Timing { ms: [33.3; Timing::N], at: 0, late: 0, frames: 0 };
    let (mut gpu_ms, mut select_ms, mut list_ms, mut ge_ms, mut ui_ms) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut last_swap = sceKernelGetSystemTimeLow();
    let mut last_vcount = sceDisplayGetVcount();
    let mut ticks = flight.pace;
    let mut frame = 0u32;
    let (mut late_last, mut list_now) = ([0.0f32; 5], 0.0f32);
    let ms = |from: u32| sceKernelGetSystemTimeLow().wrapping_sub(from) as f32 / 1000.0;

    loop {
        // -------------------------------------------------------------- input and the flight
        sceCtrlPeekBufferPositive(&mut data, 1);
        store::control(|text| {
            flight.control(text);
            remote(text, ui, &mut session);
        });
        gfx.option = flight.number(name(b"option"), 0.0) as u32;
        // What the interface asked for since the last frame, then the frame's step.
        if let Some(text) = session.obey(&mut flight, |_, _| {}) {
            prefs = Some(text);
        }
        // What it asked to have kept goes to the storage thread, which writes it beside the pack.
        if let Some(text) = prefs.take() {
            if !store::keep(&text) {
                prefs = Some(text);
            }
        }
        session.run(&mut flight, &read_pad(&data), ticks as f32 / 60.0, |x, z| gfx.height(x, z));
        // A list over the city is pixels the GE blends on top of it: the title's costs it 1.9 ms and the
        // menu's 6.7 ms, which the city gives up in triangles while one is up (`budget=` on the control
        // line sets what a flight alone may draw).
        if flight.governor.budget != applied {
            budget = flight.governor.budget;
        }
        applied = budget.saturating_sub(match session.mode {
            Mode::Title if ui.up() => TITLE_LESS,
            Mode::Menu if ui.up() => MENU_LESS,
            _ => 0,
        });
        flight.governor.budget = applied;
        {
            let state = &mut channel().state;
            session.publish(&flight, state);
            // The statistics line twice a second, while its setting is on.
            if !flight.stats {
                state.stats.clear();
            } else if frame % 15 == 0 {
                let avg = (timing.avg() * 10.0) as u32;
                let fps = 100_000 / avg.max(1);
                state.stats.clear();
                let _ = write!(state.stats, "{}.{} fps · {}.{} ms · late {} · {} draws · {}.{}k tris", fps / 10, fps % 10, avg / 10, avg % 10, timing.late, draws_last, drawn_last / 1000, drawn_last % 1000 / 100);
            }
        }
        // The interface's turn, while the GE draws the previous frame: the pad as PocketJS's hosts
        // pass it (the pad's own bits, the stick packed x then y). `option=512` withholds the turn,
        // for measuring what it costs.
        let tu = sceKernelGetSystemTimeLow();
        if gfx.option & 512 == 0 {
            ui.turn(ticks as f32 / 60.0, data.buttons.bits(), (data.lx as u32) << 8 | data.ly as u32, &session);
        }
        let turn_now = ms(tu);
        ui_ms = ui_ms * 0.9 + turn_now * 0.1;
        // The frame's draws are chosen while the GE draws the frame before.
        gfx.choose(&flight, &mut streamer);

        // -------------------------------------------------------------- the previous frame leaves the GE
        let t1 = sceKernelGetSystemTimeLow();
        sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
        let wait_now = ms(t1);
        gpu_ms = gpu_ms * 0.9 + wait_now * 0.1;
        // The GE has read the vertices of the interface's last list.
        pocketjs_psp::ge::reset_pool();
        if frame == shot_at.wrapping_add(1) {
            store::write_shot(&gfx::pixels(gfx::DRAW_BUFFER));
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
        gfx::swap();
        let vcount = sceDisplayGetVcount();
        let refreshes = vcount.wrapping_sub(last_vcount);
        // A late frame catches up, by one refresh at most.
        ticks = refreshes.clamp(1, pace + 1);
        last_vcount = vcount;
        let now = sceKernelGetSystemTimeLow();
        timing.push(now.wrapping_sub(last_swap) as f32 / 1000.0, refreshes > pace);
        if refreshes > pace {
            // What the late frame spent: its length, the interface's turn, choosing its draws, the wait for the
            // GE, and the list written before those.
            late_last = [now.wrapping_sub(last_swap) as f32 / 1000.0, turn_now, gfx.stats.select as f32 / 1000.0, wait_now, list_now];
        }
        last_swap = now;

        // -------------------------------------------------------------- this frame's list
        let t2 = sceKernelGetSystemTimeLow();
        gfx.draw(&flight, &mut streamer, ui);
        let s = gfx.stats;
        // `profile=1`: wait for the GE here, to time a frame's list from its start to its end.
        if flight.number(name(b"profile"), 0.0) != 0.0 {
            sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            ge_ms = ge_ms * 0.9 + ms(t2) * 0.1;
        }
        select_ms = select_ms * 0.9 + s.select as f32 * 0.0001;
        list_ms = list_ms * 0.9 + s.list as f32 * 0.0001;
        list_now = s.list as f32 / 1000.0;
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
            let recent = ui.recent();
            let _ = write!(
                status,
                "{{\"target\":\"psp\",\"stage\":\"running\",\"frames\":{},\"frameMs\":{:.2},\"worstMs\":{:.2},\"late\":{},\"lastLate\":{{\"ms\":{:.1},\"turn\":{:.1},\"select\":{:.1},\"gpuWait\":{:.1},\"list\":{:.1}}},\"tris\":[{},{},{},{}],\"level\":[{},{},{}],\"drawn\":{},\"draws\":{},\"places\":[{},{},{}],\"turned\":{},\"binds\":{},\"mixes\":{},\"gpuMs\":{:.2},\"geMs\":{:.2},\"cpuMs\":{{\"select\":{:.2},\"list\":{:.2}}},\"words\":{},\"reach\":[{:.0},{:.0}],\"governor\":{{\"on\":{},\"budget\":{},\"scale\":{:.3}}},\"clock\":{{\"hour\":{:.3},\"rate\":{:.3}}},\"tour\":{{\"on\":{},\"at\":{:.1},\"seconds\":{:.1}}},\"eye\":[{:.1},{:.1},{:.1}],\"look\":[{:.3},{:.3},{:.3}],\"memory\":{{\"freeAtStart\":{},\"freeAfterLoad\":{},\"free\":{},\"resident\":{},\"slot\":{},\"arena\":{{\"capacity\":{},\"afterLoad\":{},\"now\":{}}},\"kernelBlocks\":{}}},\"cells\":{{\"ready\":{},\"wanted\":{},\"reads\":{},\"readKb\":{},\"readMs\":{},\"shaded\":{},\"shadeMs\":{}}},\"shadows\":{{\"sweeps\":{},\"sweepMs\":{},\"blocksMs\":{}}},\"loadMs\":{},\"pack\":{},\"mode\":\"{}\",\"interface\":{{\"up\":{},\"error\":\"{}\",\"turns\":{},\"turnMs\":{{\"script\":{:.2},\"layout\":{:.2},\"worst\":{:.2},\"recent\":{:.2},\"recentParts\":[{:.1},{:.1},{:.1}]}},\"collections\":{},\"collectMs\":{:.1},\"cpuMs\":{:.2},\"words\":{},\"bytes\":{},\"scriptBytes\":{}}}}}",
                timing.frames,
                timing.avg(),
                timing.worst(),
                timing.late,
                late_last[0],
                late_last[1],
                late_last[2],
                late_last[3],
                late_last[4],
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
                arena::stats().capacity_bytes,
                arena_after_load,
                arena::stats().bump_bytes,
                core::ptr::addr_of!(mem::KERNEL_BYTES).read(),
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
                file.bytes,
                session.mode.name(),
                ui.up(),
                ui.error,
                ui.turns,
                ui.script_ms,
                ui.layout_ms,
                ui.worst_ms,
                recent[0] + recent[1],
                recent[0],
                recent[1],
                recent[2],
                ui.collections,
                ui.collect_ms,
                ui_ms,
                ui.words(),
                ui.bytes,
                ui.script_bytes()
            );
            store::publish(&status);
        }
    }
}
