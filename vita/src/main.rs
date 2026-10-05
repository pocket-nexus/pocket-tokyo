//! Pocket Tokyo on PS Vita.
//!
//! A frame at 960 × 544, sixty times a second: the sky, the tiles of the
//! city at the level of detail their distance asks for, the night's glow,
//! then the interface. Every 2D pixel is the interface's: the PocketJS guest
//! of `ui/` (`interface.rs`). What moves is `tokyo_sim::flight::Flight`, and
//! the flow around it (the title, the flight, the menu) is
//! `tokyo_interface::Session`, as on every other device.
//!
//! Development loop over PocketJS's wired debug transport: the pack is
//! copied once from the USB share (`host0:tokyo/city.pack`) to the memory
//! card and read from there, the interface's bundle is read from the share
//! (`host0:tokyo/tokyo.js`, `tokyo.pak`), `host0:tokyo/control.json` steers
//! the run, and status receipts carry frame timings under `engine`.

mod cars;
mod city;
mod gpu;
mod hostfs;
mod interface;
mod paths;
mod post;
mod shadow;

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use city::{CityGpu, PackFile, Show};
use gpu::Gpu;
use pocket_vita_gxm::mem::{Arena, Kind};
use pocket_vita_gxm::target::{Fence, Msaa};
use pocketjs_vita::{dev, dev_protocol::Op, devmenu::Action, graphics, input};
use post::{Look, Post};
use serde_json::{json, Value};
use tokyo_interface::{channel, pad, Command, Mode, Pad, Session};
use tokyo_sim::camera::Camera;
use tokyo_sim::flight::Flight;
use tokyo_sim::mat;
use tokyo_sim::math::*;
use tokyo_sim::sky;
use vita2d_sys as g;

#[no_mangle]
#[used]
pub static sceUserMainThreadStackSize: u32 = 1024 * 1024;

#[no_mangle]
#[used]
pub static _newlib_heap_size_user: u32 = 48 * 1024 * 1024;

extern "C" {
    fn scePowerSetArmClockFrequency(freq: i32) -> i32;
    fn scePowerSetBusClockFrequency(freq: i32) -> i32;
    fn scePowerSetGpuClockFrequency(freq: i32) -> i32;
    fn scePowerSetGpuXbarClockFrequency(freq: i32) -> i32;
    fn scePowerGetArmClockFrequency() -> i32;
    fn scePowerGetGpuClockFrequency() -> i32;
    fn sceDisplayGetVcount() -> i32;
    fn sceDisplayWaitVblankStart() -> i32;
    fn sceKernelPowerTick(kind: i32) -> i32;
}

/// Bytes of the parameter buffer: where GXM keeps a scene's transformed vertices until its tiles are drawn. When
/// a scene fills it, GXM draws what it has and starts again, at several times the cost. The default is 16 MB,
/// which the city's vertices fill at about 150 000 triangles in view.
const PARAMETER_BUFFER: u32 = 32 * 1024 * 1024;

extern "C" {
    fn __real_sceGxmInitialize(params: *const g::SceGxmInitializeParams) -> i32;
}

/// vita2d's start of GXM, with the city's parameter buffer (see `build.rs`).
///
/// # Safety
/// Called by vita2d in place of `sceGxmInitialize`, with its parameters.
#[no_mangle]
pub unsafe extern "C" fn __wrap_sceGxmInitialize(params: *const g::SceGxmInitializeParams) -> i32 {
    let mut p = *params;
    p.parameterBufferSize = PARAMETER_BUFFER;
    __real_sceGxmInitialize(&p)
}

/// Seconds a frame may wait for the GPU to finish the frame before last, before the governor takes the
/// levels of detail in. The GPU can run most of a refresh behind this thread and still show every frame
/// on time; at a third of a refresh there are a dozen frames left to act in.
const BEHIND: f32 = 0.006;

/// Samples per pixel of the scene target.
const DEFAULT_MSAA: u64 = 4;

/// vita2d's pool of temporary vertices, which the interface and the Devkit
/// menu draw from. A frame takes half of it, in turn (see the display scene).
const POOL_BYTES: u32 = 2 * 1024 * 1024;

/// The day of the year the sun follows (5 October).
const DAY: f32 = 277.0;

/// ARM, bus, GPU and GPU crossbar clocks (MHz) the frame budget assumes.
const CLOCKS: [i32; 4] = [444, 222, 222, 166];

unsafe fn set_clocks() {
    scePowerSetArmClockFrequency(CLOCKS[0]);
    scePowerSetBusClockFrequency(CLOCKS[1]);
    scePowerSetGpuClockFrequency(CLOCKS[2]);
    scePowerSetGpuXbarClockFrequency(CLOCKS[3]);
}

// Vita controller bits.
const P_SELECT: u32 = 0x1;
const P_START: u32 = 0x8;
const P_UP: u32 = 0x10;
const P_RIGHT: u32 = 0x20;
const P_DOWN: u32 = 0x40;
const P_LEFT: u32 = 0x80;
const P_L: u32 = 0x100 | 0x400;
const P_R: u32 = 0x200 | 0x800;
const P_TRIANGLE: u32 = 0x1000;
const P_CIRCLE: u32 = 0x2000;
const P_CROSS: u32 = 0x4000;
const P_SQUARE: u32 = 0x8000;

unsafe fn text(font: *mut g::vita2d_pgf, x: i32, y: i32, color: u32, scale: f32, s: &str) {
    let c = std::ffi::CString::new(s.replace('\0', " ")).unwrap();
    g::vita2d_pgf_draw_text(font, x, y, color, scale, c.as_ptr());
}

/// What a frame needs before there is a city to draw: the interface, the
/// wired-debug host and a count of frames shown.
struct Shell {
    ui: interface::Ui,
    dev: dev::Host,
    /// The system font, loaded when a frame has no interface to draw.
    font: *mut g::vita2d_pgf,
    frame: u32,
}

impl Shell {
    /// A frame of the interface alone, while the pack is copied and read (`Mode::Loading`,
    /// `message` the step) or after the start failed (`Mode::Error`, `message` the reason): the
    /// guest turns, then draws. It also publishes status, so the computer sees the new process
    /// come up.
    unsafe fn frame(&mut self, mode: Mode, message: &str) {
        let state = &mut channel().state;
        state.mode = mode;
        if state.message != message {
            state.message.clear();
            state.message.push_str(message);
        }
        self.ui.turn(interface::TURN, &interface::NEUTRAL, true, None);
        if !self.ui.live() && self.font.is_null() {
            self.font = g::vita2d_load_default_pgf();
        }
        graphics::begin_frame(0xff11_0c09);
        if self.ui.live() {
            self.ui.draw();
        } else {
            // No interface: the system font says what is going on.
            text(self.font, 48, 80, 0xffff_ffff, 1.4, "Pocket Tokyo");
            let chars: Vec<char> = message.chars().collect();
            for (i, line) in chars.chunks(90).take(4).enumerate() {
                text(self.font, 48, 130 + i as i32 * 28, 0xffd0_d0d0, 1.0, &line.iter().collect::<String>());
            }
        }
        self.dev.overlay();
        graphics::present();
        self.dev.engine = json!({"stage": if mode == Mode::Error { "error" } else { "loading" }, "message": message, "interface": {"up": self.ui.live(), "open": channel().is_open(), "error": self.ui.error}});
        self.dev.publish(self.frame, "tokyo");
        serve(&mut self.dev, self.frame, Action::None);
        self.frame += 1;
    }

    /// The start failed: the reason stays on the screen and in the status receipt.
    unsafe fn fail(&mut self, error: String) -> ! {
        pocketjs_vita::vita_log(format_args!("tokyo: {error}"));
        loop {
            self.frame(Mode::Error, &error);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Copies `from` to `to` through a temporary file, a megabyte at a time, with a frame of the
/// loading screen after each: `said(megabytes done, megabytes a second)` is its line.
unsafe fn copy(from: &str, to: &str, shell: &mut Shell, said: impl Fn(f32, f32) -> String) -> Result<usize, String> {
    let mut src = std::fs::File::open(from).map_err(|e| format!("{from}: {e}"))?;
    let temp = format!("{to}.part");
    let mut dst = std::fs::File::create(&temp).map_err(|e| format!("{temp}: {e}"))?;
    let mut chunk = vec![0u8; 1024 * 1024];
    let (mut done, t) = (0usize, Instant::now());
    loop {
        let n = src.read(&mut chunk).map_err(|e| format!("{from}: {e}"))?;
        if n == 0 {
            break;
        }
        dst.write_all(&chunk[..n]).map_err(|e| format!("{temp}: {e}"))?;
        done += n;
        let mb = done as f32 / 1e6;
        shell.frame(Mode::Loading, &said(mb, mb / t.elapsed().as_secs_f32().max(0.001)));
    }
    drop(dst);
    let _ = std::fs::remove_file(to);
    std::fs::rename(&temp, to).map_err(|e| format!("{to}: {e}"))?;
    Ok(done)
}

/// Brings the pack on the computer to the memory card when it is not the one already there, and says which
/// pack to read.
unsafe fn sync_pack(live: bool, shell: &mut Shell) -> Result<&'static str, String> {
    if live {
        let _ = std::fs::create_dir_all(paths::DATA);
        let host = hostfs::read(paths::PACK_HOST_ID, 4096);
        let card = std::fs::read(paths::PACK_CARD_ID).ok();
        if let Some(id) = host {
            if Some(&id) != card.as_ref() {
                let total = serde_json::from_slice::<Value>(&id).ok().and_then(|v| v["bytes"].as_u64()).unwrap_or(0) as f32 / 1e6;
                copy(paths::PACK_HOST, paths::PACK_CARD, shell, |mb, rate| format!("Copying the city to the memory card: {mb:.0} of {total:.0} MB at {rate:.1} MB/s"))?;
                std::fs::write(paths::PACK_CARD_ID, &id).map_err(|e| format!("{}: {e}", paths::PACK_CARD_ID))?;
            }
        }
    }
    // A development build reads the copy on the card; a package reads its own pack, whatever an earlier
    // development build left on the card.
    let first = if live { [paths::PACK_CARD, paths::PACK_APP] } else { [paths::PACK_APP, paths::PACK_CARD] };
    Ok(if std::fs::File::open(first[0]).is_ok() { first[0] } else { first[1] })
}

/// Copies a file the computer has put in the share's `outbox` to this app's folder on the memory card (a
/// package, for VitaShell to install), and leaves `<name>.done` beside the original: what was copied, or why not.
unsafe fn fetch(name: &str, shell: &mut Shell) {
    let (from, to) = (format!("{}/outbox/{name}", paths::HOST), format!("{}/{name}", paths::DATA));
    let _ = std::fs::create_dir_all(paths::DATA);
    let said = match copy(&from, &to, shell, |mb, rate| format!("Copying {name} to the memory card: {mb:.0} MB at {rate:.1} MB/s")) {
        Ok(bytes) => format!("{bytes} bytes at {to}"),
        Err(e) => format!("failed: {e}"),
    };
    let _ = hostfs::write(&format!("{from}.done"), said.as_bytes());
    // The last of those frames drew from the start of vita2d's pool: it leaves the GPU before the next
    // frame of the city writes there.
    g::vita2d_wait_rendering_done();
}

/// Remote control: `host0:tokyo/control.json`, polled off the render thread.
fn control_watcher() -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new().name("tokyo-control".into()).stack_size(256 * 1024).spawn(move || {
        // What is there at launch is left over from an earlier run: only changes after it count.
        let path = format!("{}/control.json", paths::HOST);
        let mut last = hostfs::read(&path, 64 * 1024).unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_millis(300));
            if let Some(bytes) = hostfs::read(&path, 64 * 1024) {
                if bytes != last {
                    last = bytes.clone();
                    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                        if tx.send(v).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    rx
}

/// What this device sets beside the flight (`Flight` has the eye, the tour, the clock and the
/// switches every device shares).
struct Settings {
    /// Wait for the GPU after each frame and time it.
    profile: bool,
    show: Show,
    /// Display refreshes per frame: 1 is sixty frames a second.
    pace: i32,
    look: Look,
    haze: f32,
    /// Strength of the lamp light on the ground.
    lamps: f32,
    /// Triangles a frame may draw: the distances of the levels of detail follow it.
    budget: u32,
    govern: bool,
    /// The near and the mid distance when the budget allows them in full.
    reach: (f32, f32),
    /// Draw the interface (off for a measurement of what it costs).
    interface: bool,
}

/// A control message's `press`: a mask of PocketJS button bits (the pad's own), or names.
fn press(ui: &mut interface::Ui, v: &Value) {
    if let Some(mask) = v.as_u64() {
        ui.press(mask as u32);
    }
    for name in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        ui.press(match name {
            "up" => P_UP,
            "right" => P_RIGHT,
            "down" => P_DOWN,
            "left" => P_LEFT,
            "l" => 0x100,
            "r" => 0x200,
            "triangle" => P_TRIANGLE,
            "circle" => P_CIRCLE,
            "cross" => P_CROSS,
            "square" => P_SQUARE,
            "start" => P_START,
            "select" => P_SELECT,
            _ => continue,
        });
    }
}

/// A message from the computer: `{"mode": "title" | "flight" | "menu"}` sets the flow outright,
/// `{"ui": "tour" | "fly" | "menu" | "resume" | "title"}` asks what the interface would ask,
/// `{"press": 8}` or `{"press": ["down", "circle"]}` presses buttons on the interface and
/// `{"tap": [x, y]}` taps its panel (480 × 272); the rest writes the flight's fields and this
/// device's settings.
fn apply_control(v: &Value, s: &mut Settings, flight: &mut Flight, session: &mut Session, ui: &mut interface::Ui) {
    if v["restart"] == Value::Bool(true) {
        flight.tour_at = 0.0;
    }
    let flag = |k: &str, cur: bool| v[k].as_bool().unwrap_or(cur);
    let num = |k: &str, cur: f32| v[k].as_f64().map(|x| x as f32).unwrap_or(cur);
    // `at`: seconds into the tour.
    flight.tour_at = num("at", flight.tour_at);
    flight.stats = flag("stats", flight.stats);
    s.profile = flag("profile", s.profile);
    s.interface = flag("interface", s.interface);
    s.show.top = flag("top", s.show.top);
    s.show.wall = flag("wall", s.show.wall);
    s.show.solid = flag("solid", s.show.solid);
    s.show.sectors = flag("sectors", s.show.sectors);
    if let Some(x) = v["chop"].as_u64() {
        s.show.chop = x as u32;
    }
    // `near` and `mid` set the distances outright and stop the governor; `budget` hands them back to it.
    if v.get("near").is_some() || v.get("mid").is_some() {
        s.show.near = num("near", s.show.near);
        s.show.mid = num("mid", s.show.mid);
        s.govern = false;
    }
    if let Some(x) = v["budget"].as_u64() {
        s.budget = x as u32;
        s.govern = true;
    }
    if let (Some(a), Some(b)) = (v["reach"][0].as_f64(), v["reach"][1].as_f64()) {
        s.reach = (a as f32, b as f32);
    }
    // The flow first: a flight's own switches below are then written into the mode they belong to.
    let asked = match (v["ui"].as_str(), session.mode) {
        (Some("tour"), Mode::Title) => Some(Command::Start { tour: true }),
        (Some("fly"), Mode::Title) => Some(Command::Start { tour: false }),
        (Some("tour"), _) => Some(Command::Tour(true)),
        (Some("fly"), _) => Some(Command::Tour(false)),
        (Some("menu"), _) => Some(Command::Menu(true)),
        (Some("resume"), _) => Some(Command::Menu(false)),
        (Some("title"), _) => Some(Command::Title),
        _ => None,
    };
    if let Some(command) = asked {
        session.command(flight, command);
    }
    if let Some(mode) = v["mode"].as_str().and_then(Mode::parse).filter(|m| !matches!(m, Mode::Loading | Mode::Error)) {
        session.mode = mode;
    }
    if v["gc"] == Value::Bool(true) {
        // Safety: the render thread, between two turns.
        unsafe { ui.collect() };
    }
    press(ui, &v["press"]);
    if let (Some(x), Some(y)) = (v["tap"][0].as_u64(), v["tap"][1].as_u64()) {
        ui.tap(x as u16, y as u16);
    }
    flight.tour_on = flag("tour", flight.tour_on);
    flight.traffic = flag("traffic", flight.traffic);
    flight.hour = num("hour", flight.hour);
    flight.rate = num("rate", flight.rate);
    s.haze = num("haze", s.haze);
    s.lamps = num("lamps", s.lamps);
    if let Some(x) = v["pace"].as_i64() {
        s.pace = (x as i32).clamp(1, 4);
    }
    let post = &v["post"];
    if post.is_object() {
        let l = &mut s.look;
        l.bloom = post["bloom"].as_bool().unwrap_or(l.bloom);
        for (key, slot) in [("threshold", &mut l.threshold), ("bloomGain", &mut l.bloom_gain)] {
            if let Some(x) = post[key].as_f64() {
                *slot = x as f32;
            }
        }
    }
    let f = |a: &Value, i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    if v.get("view").is_some() {
        flight.view = match (&v["view"]["pos"], &v["view"]["target"]) {
            (p, t) if p.is_array() && t.is_array() => Some((v3(f(p, 0), f(p, 1), f(p, 2)), v3(f(t, 0), f(t, 1), f(t, 2)), v["view"]["fov"].as_f64().unwrap_or(55.0) as f32)),
            _ => None,
        };
    }
    // `fly`: put the free camera somewhere and leave it to the pad.
    let (p, t) = (&v["fly"]["pos"], &v["fly"]["target"]);
    if p.is_array() && t.is_array() {
        flight.cam = Camera::looking(v3(f(p, 0), f(p, 1), f(p, 2)), v3(f(t, 0), f(t, 1), f(t, 2)));
        flight.view = None;
        flight.tour_on = false;
    }
}

/// The pad as the flow reads it: both sticks, ✕ to fly faster, R and L to climb and descend, SELECT
/// for the tour, left and right on the d-pad for the clock, and START for when no interface is on
/// the screen.
/// Display refreshes from the count `then` to the count `now`. The display's counter has 16 bits: it starts
/// again at 0 after 65 536 refreshes (18 minutes), and a plain difference across that point is -65 535, which
/// a loop that waits for one refresh takes 18 more minutes to see pass.
fn refreshes(now: i32, then: i32) -> i32 {
    now.wrapping_sub(then) & 0xffff
}

fn session_pad(p: &input::Pad) -> Pad {
    let mut b = 0;
    for (bit, to) in [(P_CROSS, pad::FAST), (P_R, pad::UP), (P_L, pad::DOWN), (P_SELECT, pad::TOUR), (P_LEFT, pad::EARLIER), (P_RIGHT, pad::LATER), (P_START, pad::MENU)] {
        if p.buttons & bit != 0 {
            b |= to;
        }
    }
    let axis = |v: u8| {
        let x = (v as f32 - 127.5) / 127.5;
        // The sticks rest off centre by up to a fifth of their travel: nothing inside that counts, and the rest is rescaled.
        const DEAD: f32 = 0.24;
        if abs(x) < DEAD {
            0.0
        } else {
            (x - DEAD * if x < 0.0 { -1.0 } else { 1.0 }) / (1.0 - DEAD)
        }
    };
    Pad { buttons: b, lx: axis(p.lx), ly: -axis(p.ly), rx: axis(p.rx), ry: -axis(p.ry) }
}

/// Rolling frame statistics over the last `N` frames.
struct Timing {
    ms: [f32; Timing::N],
    at: usize,
    late: u32,
    frames: u32,
}

impl Timing {
    const N: usize = 240;
    fn push(&mut self, ms: f32, late: bool) {
        self.ms[self.at] = ms;
        self.at = (self.at + 1) % Self::N;
        self.frames += 1;
        if late {
            self.late += 1;
        }
    }
    fn avg(&self) -> f32 {
        self.ms.iter().sum::<f32>() / Self::N as f32
    }
    fn worst(&self) -> f32 {
        self.ms.iter().fold(0.0, |a, &b| a.max(b))
    }
}

fn main() {
    unsafe {
        // Development builds take boot switches from the USB share:
        // {"msaa": 0 | 2 | 4, "title": false, "ground": 1024, "mode": "flight", "tour": false, "pace": 1}.
        let live = cfg!(feature = "usb-debug");
        let boot: Value = if live { hostfs::read(&format!("{}/boot.json", paths::HOST), 4096).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null) } else { Value::Null };
        let samples = boot["msaa"].as_u64().unwrap_or(DEFAULT_MSAA);
        // The scene target is multisampled; the display surface it is composed onto is not.
        let msaa = match samples {
            4 => Msaa::X4,
            2 => Msaa::X2,
            _ => Msaa::None,
        };
        // The Pocket3D title card plays before the renderer starts. A development build skips it with {"title": false}.
        if !(live && boot["title"] == Value::Bool(false)) {
            pocket3d_title::vita::play();
        }
        // The display's own surface is multisampled: the scene is drawn straight into it. (The host's own start of
        // vita2d finds it started.)
        let display_msaa = match msaa {
            Msaa::X4 => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_4X,
            Msaa::X2 => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_2X,
            _ => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_NONE,
        };
        if g::vita2d_init_advanced_with_msaa(POOL_BYTES, display_msaa) < 0 {
            pocketjs_vita::vita_log(format_args!("tokyo: vita2d did not start"));
            return;
        }
        if let Err(error) = graphics::init_with_pool(POOL_BYTES) {
            pocketjs_vita::vita_log(format_args!("tokyo: graphics {error}"));
            return;
        }
        set_clocks();
        input::init();
        // The interface comes up first: it shows the load, and a failure.
        let mut shell = Shell { ui: interface::Ui::boot(), dev: dev::Host::new(), font: core::ptr::null_mut(), frame: 0 };
        channel().state.prefs = std::fs::read_to_string(format!("{}/{}", paths::DATA, paths::INTERFACE_FILE)).unwrap_or_default();

        // ------------------------------------------------------------------ load
        let t_load = Instant::now();
        shell.frame(Mode::Loading, "Looking for the city");
        let pack_path = match sync_pack(live, &mut shell) {
            Ok(p) => p,
            Err(e) => shell.fail(e),
        };
        let copy_ms = t_load.elapsed().as_millis() as u64;
        let loaded = (|| -> Result<_, String> {
            let mut p = PackFile::open(pack_path)?;
            let meta: Value = serde_json::from_slice(&p.read(tokyo_pack::META)?).map_err(|e| e.to_string())?;
            let mut gpu = Gpu::new(live)?;
            // Video memory in blocks of 4 MiB, so that the last block leaves at most that much unused. The
            // interface's glyph pages are there too, one byte a texel (PocketJS's Vita host), beside its map:
            // 4 MB, which leaves the city room for ground pictures of 1024 texels (65 MB; 69 MB reserved in
            // all). What the GPU samples stays in video memory: with the shadow targets in GPU-mapped main
            // memory a night tour had 26 late frames in 30 s.
            let mut vram = Arena::new(Kind::Cdram, 4 * 1024 * 1024);
            let mut targets = Arena::new(Kind::Main, 4 * 1024 * 1024);
            let post = Post::new(&mut gpu, &mut vram, &mut targets, msaa)?;
            let ground_top = boot["ground"].as_u64().unwrap_or(1024) as u32;
            let city = CityGpu::load(&mut p, &mut gpu, &mut vram, msaa.gxm(), ground_top, |line| shell.frame(Mode::Loading, line))?;
            let cars = cars::Cars::new(&mut gpu, &city::scene_defines(&city.city), msaa.gxm())?;
            gpu.finish();
            shell.frame(Mode::Loading, "Casting the first shadows");
            let c = city.city;
            let shadows = shadow::Shadows::new(&mut vram, city.heights.clone(), c.grid_w as usize, c.grid_h as usize, c.grid_step, c.height_step, sky::light(c.hour, DAY).dir)?;
            Ok((meta, gpu, city, vram, post, targets, shadows, cars))
        })();
        let (meta, gpu, mut city, vram, mut post, _targets, mut shadows, mut cars) = match loaded {
            Ok(x) => x,
            Err(e) => shell.fail(e),
        };
        let load_ms = t_load.elapsed().as_millis() as u64;
        // Which pack this is: the record that came with it.
        let pack_id: Value = std::fs::read(paths::PACK_CARD_ID).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);

        let lanes = core::mem::take(&mut city.lanes);
        let mut traffic = tokyo_sim::traffic::Traffic::new(lanes.0, lanes.1, 800, 0x70_6b79);
        let mut ring = match pocket_vita_gxm::mem::Ring::new(cars::Cars::frame_bytes() + 4096, 2) {
            Ok(r) => r,
            Err(e) => shell.fail(e),
        };
        let mut fence = Fence::new(0, 2);
        let mut scene_fence = Fence::new(2, 2);
        let control = if live { control_watcher() } else { mpsc::channel().1 };
        let mut set = Settings {
            profile: false,
            show: Show { top: true, wall: true, solid: true, near: 300.0, mid: 1300.0, sectors: true, chop: 1 },
            pace: boot["pace"].as_i64().unwrap_or(1) as i32,
            look: Look::DEFAULT,
            haze: 0.00022,
            lamps: 1.0,
            budget: 200_000,
            govern: true,
            reach: (300.0, 1300.0),
            interface: true,
        };
        // The eye, the tour and the clock, as on every device. The distances of the levels of detail stay with
        // this device's own governor below, which counts what the city's three passes drew.
        let mut flight = Flight::new(city.city.view, city.city.hour, city.tour.clone(), set.budget, set.reach, 1);
        flight.governor.on = false;
        // The flow: the title over the tour, the flight, the menu. This device draws traffic, so the interface
        // may turn it off.
        let mut session = Session::new();
        session.traffic = true;
        if let Some(mode) = boot["mode"].as_str().and_then(Mode::parse).filter(|m| !matches!(m, Mode::Loading | Mode::Error)) {
            session.mode = mode;
        }
        flight.tour_on = boot["tour"].as_bool().unwrap_or(true);
        channel().state.message.clear();
        let mut scale = 1.0f32;
        // Frames the distances stay in after the GPU fell behind.
        let mut hold = 0u32;
        // A list was up at the last frame (the title is one).
        let mut listed = true;

        let ctx = g::vita2d_get_context();
        let mut timing = Timing { ms: [16.7; Timing::N], at: 0, late: 0, frames: 0 };
        let mut last = Instant::now();
        let mut last_vcount = sceDisplayGetVcount();
        let (mut draw_ms, mut gpu_ms, mut scene_ms, mut ui_ms, mut ui_draw_ms) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut stats = city::Stats::default();
        let mut clock_tick = 0u32;
        let mut seconds = 0.0f32;
        let mut frame_no = shell.frame;
        // Late frames by what held them: this thread's own work (the flight, the interface's turn, the draws)
        // took most of a refresh, or the GPU had not finished an earlier frame.
        let (mut late_cpu, mut late_gpu) = (0u32, 0u32);
        // The last loading frame drew from the start of vita2d's pool: it leaves the GPU before the first frame
        // of the loop writes there.
        g::vita2d_wait_rendering_done();
        // What mounting the screens left behind is collected before the first frame. After that the guest is
        // collected when the title comes back, and at every 256th list besides: its collector takes 35 ms and
        // does not start by itself (`interface.rs`).
        shell.ui.collect();
        let mut lists = 0u32;
        let mut mode_was = session.mode;

        loop {
            // -------------------------------------------------------------- input and the flight
            let t_frame = Instant::now();
            let raw = input::read();
            let (buttons, action) = shell.dev.menu.input(raw.buttons);
            // While the Devkit menu is up the pad is its own.
            let menu = shell.dev.menu.visible;
            let pad = if menu { interface::NEUTRAL } else { input::Pad { buttons, ..raw } };
            while let Ok(v) = control.try_recv() {
                if let Some(name) = v["fetch"].as_str().filter(|n| !n.contains('/') && !n.contains("..")) {
                    shell.frame = frame_no;
                    fetch(name, &mut shell);
                    frame_no = shell.frame;
                }
                apply_control(&v, &mut set, &mut flight, &mut session, &mut shell.ui);
            }
            // What the interface asked for on its last turn; what it wants kept goes to the card.
            if let Some(text) = session.obey(&mut flight, |_, _| {}) {
                paths::write_text(paths::INTERFACE_FILE, &text);
                channel().state.prefs = text;
            }
            if session.mode != mode_was {
                if session.mode != Mode::Flight {
                    lists += 1;
                }
                if session.mode == Mode::Title || (session.mode == Mode::Menu && lists % 256 == 0) {
                    shell.ui.collect();
                }
                mode_was = session.mode;
            }
            let vcount = sceDisplayGetVcount();
            let vblanks = refreshes(vcount, last_vcount).clamp(1, 4);
            last_vcount = vcount;
            let dt = vblanks as f32 / 60.0;
            seconds += dt;
            session.run(&mut flight, &session_pad(&pad), dt, |x, z| city.height(x, z));
            if flight.traffic {
                traffic.step(dt);
            }

            // -------------------------------------------------------------- the interface
            // It is shown the flight and the settings as they now are, then takes its turn.
            let tu = Instant::now();
            {
                let state = &mut channel().state;
                session.publish(&flight, state);
                if !flight.stats {
                    state.stats.clear();
                } else if frame_no % 30 == 0 {
                    state.stats = format!("{:.1} fps · {:.1} ms · late {} · {} draws · {}k tris", 1000.0 / timing.avg().max(0.1), timing.avg(), timing.late, stats.draws, (stats.tris[0] + stats.tris[1] + stats.tris[2]) / 1000);
                }
            }
            shell.ui.turn(dt, &pad, menu, Some(&session));
            ui_ms = ui_ms * 0.9 + tu.elapsed().as_secs_f32() * 100.0;

            // -------------------------------------------------------------- camera and light
            let (eye, look, fov) = flight.eye();
            let aspect = 960.0 / 544.0;
            let vp = mat::mul(&mat::perspective(fov, aspect, 2.0, 12000.0), &mat::view(eye, look, 0.0));
            let light = sky::light(flight.hour, DAY);
            shadows.update(light.dir);
            let half = |a: [f32; 3], b: [f32; 3], s: f32| [(a[0] + s * b[0]) * 0.5, (a[1] + s * b[1]) * 0.5, (a[2] + s * b[2]) * 0.5];
            let (side, rise) = (half(light.sky, light.ground, 1.0), half(light.sky, light.ground, -1.0));
            #[rustfmt::skip]
            let frame: [f32; 16] = [
                light.dir.x, light.dir.y, light.dir.z, set.haze,
                side[0] / city::LIGHT_SCALE, side[1] / city::LIGHT_SCALE, side[2] / city::LIGHT_SCALE, light.night,
                rise[0] / city::LIGHT_SCALE, rise[1] / city::LIGHT_SCALE, rise[2] / city::LIGHT_SCALE, 0.0,
                0.94, seconds, 0.0, 0.0,
            ];
            let haze = light.horizon;
            let look_f: [f32; 8] = [haze[0], haze[1], haze[2], light.night * 2.0 * set.lamps, light.sun[0] / city::LIGHT_SCALE, light.sun[1] / city::LIGHT_SCALE, light.sun[2] / city::LIGHT_SCALE, 0.0];
            let sky_mvp = mat::mul(&mat::perspective(fov, aspect, 0.5, 4.0), &mat::view(V3::ZERO, look, 0.0));
            let glow = saturate(light.sun_dir.y * 6.0 + 0.6);
            #[rustfmt::skip]
            let sky_f: [f32; 16] = [
                haze[0], haze[1], haze[2], 0.0,
                light.zenith[0], light.zenith[1], light.zenith[2], 0.0,
                light.sun_dir.x, light.sun_dir.y, light.sun_dir.z, light.dark,
                (0.6 + light.sun[0]) * glow, (0.45 + light.sun[1]) * glow, (0.3 + light.sun[2]) * glow, 0.0,
            ];

            // -------------------------------------------------------------- the scene
            let t2 = Instant::now();
            let slot = (frame_no % 2) as usize;
            // This slot's vertices were last used two frames ago: the GPU must be done with them.
            fence.wait(slot);
            let waited = t2.elapsed();
            ring.next_frame();
            let c = city.city;
            let grid = (c.grid_w as f32 * c.grid_step, c.grid_h as f32 * c.grid_step);
            let world_map = [1.0 / grid.0, 1.0 / grid.1, -c.grid_x0 / grid.0, -c.grid_z0 / grid.1];
            // The glow of the frame on display, by night.
            let mut look_now = set.look;
            look_now.bloom_gain *= light.night;
            if let Err(e) = post.bloom(ctx, &look_now, g::vita2d_get_current_fb(), 960) {
                pocketjs_vita::vita_log(format_args!("tokyo: {e}"));
            }
            // The interface and the Devkit menu draw from vita2d's one pool of temporary vertices. This slot's
            // frame takes its own half, last written two frames ago, which `fence.wait(slot)` above saw out of
            // the GPU: no frame overwrites vertices still being read.
            let open_display = || {
                g::vita2d_pool_reset();
                if slot == 1 {
                    g::vita2d_pool_malloc(POOL_BYTES / 2);
                }
                g::vita2d_start_drawing_advanced(core::ptr::null_mut(), 0);
            };
            if set.profile {
                // Into the scene's own target, timed by itself, then copied to the display.
                if let Err(e) = post.begin_scene(ctx) {
                    pocketjs_vita::vita_log(format_args!("tokyo: {e}"));
                }
                post.draw_sky(ctx, &sky_mvp, &sky_f);
                stats = city.draw(ctx, &vp, eye, &frame, &look_f, shadows.texture(), &set.show);
                if flight.traffic {
                    cars.draw(ctx, &traffic, &mut ring, &vp, eye, &world_map, &frame, &look_f, shadows.texture());
                }
                post.end_scene(ctx, Some(scene_fence.signal(slot)));
                let tg = Instant::now();
                scene_fence.wait(slot);
                scene_ms = scene_ms * 0.9 + tg.elapsed().as_secs_f32() * 100.0;
                open_display();
                post.copy_scene(ctx);
            } else {
                open_display();
                post.full_view(ctx);
                post.draw_sky(ctx, &sky_mvp, &sky_f);
                stats = city.draw(ctx, &vp, eye, &frame, &look_f, shadows.texture(), &set.show);
                if flight.traffic {
                    cars.draw(ctx, &traffic, &mut ring, &vp, eye, &world_map, &frame, &look_f, shadows.texture());
                }
            }
            post.add_glow(ctx, &look_now);
            // vita2d draws (the interface, the menu) expect its own viewport and no depth.
            g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.5, 0.5);
            gpu::state_overlay(ctx, false);
            let td = Instant::now();
            if set.interface {
                shell.ui.draw();
            }
            ui_draw_ms = ui_draw_ms * 0.9 + td.elapsed().as_secs_f32() * 100.0;
            shell.dev.overlay();
            g::sceGxmEndScene(ctx, core::ptr::null(), fence.signal(slot));
            draw_ms = draw_ms * 0.9 + t2.elapsed().as_secs_f32() * 100.0;
            if set.profile {
                let tg = Instant::now();
                fence.wait(slot);
                gpu_ms = gpu_ms * 0.9 + tg.elapsed().as_secs_f32() * 100.0;
            }
            // The distances of the levels of detail follow the triangle budget: in quickly when a frame draws too
            // many, out slowly when there is room. They also come in when this frame had to wait for the GPU to
            // finish the frame before last: the GPU is then a refresh behind, which a view that fills more
            // pixels than its triangles say brings about, and the next frames would be late. After such a wait
            // the distances stay in for two seconds.
            if set.govern {
                let drawn = stats.tris[0] + stats.tris[1] + stats.tris[2];
                // A list over the city is a scrim over the whole screen and a clip, which vita2d makes of
                // passes over the whole screen's stencil: the GPU's time for them comes out of the city
                // behind the scrim.
                let listing = session.mode != Mode::Flight;
                let budget = if listing { set.budget / 4 * 3 } else { set.budget };
                // A list comes up in one frame: the distances come in with it, not ten frames after it.
                if listing && !listed {
                    scale = min(scale, 0.75);
                }
                listed = listing;
                let behind = !set.profile && waited.as_secs_f32() > BEHIND;
                if behind {
                    hold = 120;
                }
                if drawn > budget {
                    scale *= 0.97;
                } else if behind {
                    scale *= 0.985;
                } else if (drawn as f32) < budget as f32 * 0.88 && hold == 0 {
                    scale *= 1.008;
                }
                hold = hold.saturating_sub(1);
                scale = clamp(scale, 0.25, 1.0);
                set.show.near = set.reach.0 * scale;
                set.show.mid = set.reach.1 * scale;
            }
            let worked = t_frame.elapsed().saturating_sub(waited);
            g::vita2d_swap_buffers();
            // Hold the pace: a frame is shown for `pace` refreshes.
            while refreshes(sceDisplayGetVcount(), last_vcount) < set.pace {
                sceDisplayWaitVblankStart();
            }

            let now = Instant::now();
            let shown = refreshes(sceDisplayGetVcount(), last_vcount);
            timing.push((now - last).as_secs_f32() * 1000.0, shown > set.pace);
            if shown > set.pace {
                if worked.as_secs_f32() > 0.012 {
                    late_cpu += 1;
                } else {
                    late_gpu += 1;
                }
            }
            last = now;

            // -------------------------------------------------------------- status
            clock_tick += 1;
            // A tour nobody touches is still being watched: the console's own timers (dimming, the display
            // off, standby) start again.
            if clock_tick % 120 == 0 {
                sceKernelPowerTick(0);
            }
            if clock_tick % 60 == 0 {
                // The system lowers the clocks after a suspend; set them again.
                if scePowerGetArmClockFrequency() < CLOCKS[0] - 20 {
                    set_clocks();
                }
            }
            if frame_no % 10 == 0 {
                shell.dev.engine = json!({
                    "stage": "running",
                    "mode": session.mode.name(),
                    "interface": {"up": shell.ui.live(), "open": channel().is_open(), "error": shell.ui.error, "turns": shell.ui.turns, "turnMs": shell.ui.turn_ms, "worstTurnMs": shell.ui.worst_ms, "collections": shell.ui.collections, "collectMs": shell.ui.collect_ms},
                    "pack": {"path": pack_path, "sha256": pack_id["sha256"], "bytes": pack_id["bytes"], "name": meta["name"], "area": meta["area"], "profile": meta["profile"], "source": meta["source"]},
                    "loadMs": load_ms, "copyMs": copy_ms,
                    "frameMs": timing.avg(), "worstMs": timing.worst(), "late": timing.late, "lateBy": {"cpu": late_cpu, "gpu": late_gpu}, "frames": timing.frames, "pace": set.pace,
                    "cpuMs": {"draw": draw_ms, "interface": ui_ms, "interfaceDraw": ui_draw_ms},
                    "gpuMs": if set.profile { json!(gpu_ms) } else { Value::Null },
                    "sceneMs": if set.profile { json!(scene_ms) } else { Value::Null },
                    "city": {"draws": stats.draws, "tris": {"top": stats.tris[0], "wall": stats.tris[1], "solid": stats.tris[2]}, "places": stats.places, "turned": stats.turned},
                    "camera": {"pos": [eye.x, eye.y, eye.z], "look": [look.x, look.y, look.z], "fov": fov},
                    "clock": {"hour": flight.hour, "rate": flight.rate, "night": light.night},
                    "shadows": {"sweeps": shadows.sweeps, "sweepMs": shadows.sweep_ms()},
                    "cars": {"all": traffic.cars.len(), "shown": cars.shown},
                    "tour": {"on": flight.tour_on, "at": flight.tour_at, "seconds": flight.tour.seconds()}, "governor": {"on": set.govern, "budget": set.budget, "scale": scale, "held": hold},
                    "settings": {"near": set.show.near, "mid": set.show.mid, "top": set.show.top, "wall": set.show.wall, "solid": set.show.solid, "profile": set.profile, "stats": flight.stats, "traffic": flight.traffic, "invert": session.invert, "post": {"bloom": set.look.bloom}},
                    "programs": {"compiled": gpu.compiled, "cached": gpu.cached},
                    "msaa": samples,
                    "memory": {"geometry": city.geometry_bytes, "textures": city.texture_bytes, "vram": vram.reserved()},
                    "clockMhz": [scePowerGetArmClockFrequency(), scePowerGetGpuClockFrequency()],
                });
            }
            shell.dev.publish(frame_no, "tokyo");
            serve(&mut shell.dev, frame_no, action);
            frame_no = frame_no.wrapping_add(1);
        }
    }
}

/// Answers wired-debug requests at a frame boundary.
unsafe fn serve(dev: &mut dev::Host, frame: u32, action: Action) {
    let mut request = dev.poll();
    let op = request.as_ref().map(|r| r.command.op).or(match action {
        Action::Capture => Some(Op::Capture),
        _ => None,
    });
    match op {
        Some(Op::Status) => request.take().unwrap().finish(Ok(dev.status(frame, "tokyo"))),
        Some(Op::Menu) => {
            dev.menu.visible = !dev.menu.visible;
            request.take().unwrap().finish(Ok(json!({"menu": dev.menu.visible})));
        }
        Some(Op::Capture) => {
            if let Some(request) = request.take() {
                let _ = request.reply.try_send(dev.capture(frame));
            } else {
                dev.capture_from_menu(frame);
            }
        }
        Some(Op::Native) => {
            let request = request.take().unwrap();
            g::vita2d_wait_rendering_done();
            let result = dev::exec_native(request.native_path.as_ref().unwrap());
            request.finish(result.map(|_| json!({})));
        }
        Some(Op::Push | Op::Reload | Op::Reset) => {
            if let Some(request) = request.take() {
                request.finish(Err("Pocket Tokyo's interface is read from the share when the app starts; use native to start it again".into()));
            }
        }
        None => {}
    }
}
