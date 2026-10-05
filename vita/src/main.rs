//! Pocket Tokyo on PS Vita.
//!
//! A frame at 960 × 544, sixty times a second: the sky, the tiles of the
//! city at the level of detail their distance asks for, then the bright
//! chain and the interface.
//!
//! Development loop over PocketJS's wired debug transport: the pack is
//! copied once from the USB share (`host0:tokyo/city.pack`) to the memory
//! card and read from there, `host0:tokyo/control.json` steers the run, and
//! status receipts carry frame timings under `engine`.

mod cars;
mod city;
mod gpu;
mod hostfs;
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
use tokyo_sim::camera::{btn, Camera, Input};
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

/// Samples per pixel of the scene target.
const DEFAULT_MSAA: u64 = 4;

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
const P_CROSS: u32 = 0x4000;

unsafe fn text(font: *mut g::vita2d_pgf, x: i32, y: i32, color: u32, scale: f32, s: &str) {
    let c = std::ffi::CString::new(s.replace('\0', " ")).unwrap();
    g::vita2d_pgf_draw_text(font, x, y, color, scale, c.as_ptr());
}

/// A frame of the loading screen; it also publishes status, so the computer sees the new process come up.
unsafe fn loading(font: *mut g::vita2d_pgf, dev: &mut dev::Host, frame: &mut u32, lines: &[String]) {
    graphics::begin_frame(0xff1c_140e);
    text(font, 48, 80, 0xffff_ffff, 1.4, "Pocket Tokyo");
    for (i, l) in lines.iter().enumerate() {
        text(font, 48, 130 + i as i32 * 28, 0xffd0_d0d0, 1.0, l);
    }
    dev.overlay();
    graphics::present();
    dev.engine = json!({"stage": "loading", "lines": lines});
    dev.publish(*frame, "tokyo");
    serve(dev, *frame, Action::None);
    *frame += 1;
}

/// Brings the pack on the computer to the memory card when it is not the one already there, and says which
/// pack to read.
unsafe fn sync_pack(live: bool, font: *mut g::vita2d_pgf, dev: &mut dev::Host, frame: &mut u32) -> Result<&'static str, String> {
    if live {
        let _ = std::fs::create_dir_all(paths::DATA);
        let host = hostfs::read(paths::PACK_HOST_ID, 4096);
        let card = std::fs::read(paths::PACK_CARD_ID).ok();
        if let Some(id) = host {
            if Some(&id) != card.as_ref() {
                let total = serde_json::from_slice::<Value>(&id).ok().and_then(|v| v["bytes"].as_u64()).unwrap_or(0) as f32 / 1e6;
                let mut src = std::fs::File::open(paths::PACK_HOST).map_err(|e| format!("{}: {e}", paths::PACK_HOST))?;
                let temp = format!("{}.part", paths::PACK_CARD);
                let mut dst = std::fs::File::create(&temp).map_err(|e| format!("{temp}: {e}"))?;
                let mut chunk = vec![0u8; 1024 * 1024];
                let (mut done, t) = (0usize, Instant::now());
                loop {
                    let n = src.read(&mut chunk).map_err(|e| format!("{}: {e}", paths::PACK_HOST))?;
                    if n == 0 {
                        break;
                    }
                    dst.write_all(&chunk[..n]).map_err(|e| format!("{temp}: {e}"))?;
                    done += n;
                    let secs = t.elapsed().as_secs_f32().max(0.001);
                    loading(font, dev, frame, &["Copying the city to the memory card".into(), format!("{:.0} of {:.0} MB at {:.1} MB/s", done as f32 / 1e6, total, done as f32 / 1e6 / secs)]);
                }
                drop(dst);
                let _ = std::fs::remove_file(paths::PACK_CARD);
                std::fs::rename(&temp, paths::PACK_CARD).map_err(|e| format!("{}: {e}", paths::PACK_CARD))?;
                std::fs::write(paths::PACK_CARD_ID, &id).map_err(|e| format!("{}: {e}", paths::PACK_CARD_ID))?;
            }
        }
    }
    Ok(if std::fs::File::open(paths::PACK_CARD).is_ok() { paths::PACK_CARD } else { paths::PACK_APP })
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

struct Settings {
    stats: bool,
    /// Wait for the GPU after each frame and time it.
    profile: bool,
    show: Show,
    /// Display refreshes per frame: 1 is sixty frames a second.
    pace: i32,
    look: Look,
    /// A fixed camera: eye, target, vertical field of view.
    view: Option<(V3, V3, f32)>,
    /// Tokyo's clock, and how many hours pass in a second.
    hour: f32,
    rate: f32,
    haze: f32,
    /// Strength of the lamp light on the ground.
    lamps: f32,
    /// The camera follows the tour.
    tour: bool,
    traffic: bool,
    /// Triangles a frame may draw: the distances of the levels of detail follow it.
    budget: u32,
    govern: bool,
    /// The near and the mid distance when the budget allows them in full.
    reach: (f32, f32),
}

fn apply_control(v: &Value, s: &mut Settings, cam: &mut Camera, tour_at: &mut f32) {
    if v["restart"] == Value::Bool(true) {
        *tour_at = 0.0;
    }
    let flag = |k: &str, cur: bool| v[k].as_bool().unwrap_or(cur);
    let num = |k: &str, cur: f32| v[k].as_f64().map(|x| x as f32).unwrap_or(cur);
    s.stats = flag("stats", s.stats);
    s.profile = flag("profile", s.profile);
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
    s.tour = flag("tour", s.tour);
    s.traffic = flag("traffic", s.traffic);
    s.hour = num("hour", s.hour);
    s.rate = num("rate", s.rate);
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
        s.view = match (&v["view"]["pos"], &v["view"]["target"]) {
            (p, t) if p.is_array() && t.is_array() => Some((v3(f(p, 0), f(p, 1), f(p, 2)), v3(f(t, 0), f(t, 1), f(t, 2)), v["view"]["fov"].as_f64().unwrap_or(55.0) as f32)),
            _ => None,
        };
    }
    // `fly`: put the free camera somewhere and leave it to the pad.
    if let (p, t) = (&v["fly"]["pos"], &v["fly"]["target"]) {
        if p.is_array() && t.is_array() {
            *cam = Camera::looking(v3(f(p, 0), f(p, 1), f(p, 2)), v3(f(t, 0), f(t, 1), f(t, 2)));
            s.view = None;
        }
    }
}

fn pad_input(pad: &input::Pad, buttons: u32) -> Input {
    let mut b = 0;
    for (bit, to) in [(P_CROSS, btn::FAST), (P_R, btn::UP), (P_L, btn::DOWN)] {
        if buttons & bit != 0 {
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
    Input { buttons: b, lx: axis(pad.lx), ly: -axis(pad.ly), rx: axis(pad.rx), ry: -axis(pad.ry) }
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
        // Development builds take boot switches from the USB share: {"msaa": 0 | 2 | 4, "title": false, "ground": 512}.
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
        if g::vita2d_init_advanced_with_msaa(1024 * 1024, display_msaa) < 0 {
            pocketjs_vita::vita_log(format_args!("tokyo: vita2d did not start"));
            return;
        }
        if let Err(error) = graphics::init_with_pool(1024 * 1024) {
            pocketjs_vita::vita_log(format_args!("tokyo: graphics {error}"));
            return;
        }
        set_clocks();
        input::init();
        let mut dev = dev::Host::new();
        let font = g::vita2d_load_default_pgf();
        let mut frame_no = 0u32;
        let fail = |font, dev: &mut dev::Host, frame_no: &mut u32, e: String| -> ! {
            pocketjs_vita::vita_log(format_args!("tokyo: {e}"));
            loop {
                loading(font, dev, frame_no, &["Could not start.".into(), e.chars().take(90).collect(), e.chars().skip(90).take(90).collect()]);
                std::thread::sleep(Duration::from_millis(100));
            }
        };

        // ------------------------------------------------------------------ load
        let t_load = Instant::now();
        loading(font, &mut dev, &mut frame_no, &["Looking for the city".into()]);
        let pack_path = match sync_pack(live, font, &mut dev, &mut frame_no) {
            Ok(p) => p,
            Err(e) => fail(font, &mut dev, &mut frame_no, e),
        };
        let copy_ms = t_load.elapsed().as_millis() as u64;
        let loaded = (|| -> Result<_, String> {
            let mut p = PackFile::open(pack_path)?;
            let meta: Value = serde_json::from_slice(&p.read(tokyo_pack::META)?).map_err(|e| e.to_string())?;
            let mut gpu = Gpu::new(live)?;
            let mut vram = Arena::new(Kind::Cdram, 16 * 1024 * 1024);
            let mut targets = Arena::new(Kind::Main, 4 * 1024 * 1024);
            let post = Post::new(&mut gpu, &mut vram, &mut targets, msaa)?;
            let ground_top = boot["ground"].as_u64().unwrap_or(512) as u32;
            let city = CityGpu::load(&mut p, &mut gpu, &mut vram, msaa.gxm(), ground_top, |line| loading(font, &mut dev, &mut frame_no, &[line.to_string()]))?;
            let cars = cars::Cars::new(&mut gpu, &city::scene_defines(&city.city), msaa.gxm())?;
            gpu.finish();
            loading(font, &mut dev, &mut frame_no, &["Casting the first shadows".into()]);
            let c = city.city;
            let shadows = shadow::Shadows::new(&mut vram, city.heights.clone(), c.grid_w as usize, c.grid_h as usize, c.grid_step, c.height_step, sky::light(c.hour, DAY).dir)?;
            Ok((meta, gpu, city, vram, post, targets, shadows, cars))
        })();
        let (meta, gpu, mut city, vram, mut post, _targets, mut shadows, mut cars) = match loaded {
            Ok(x) => x,
            Err(e) => fail(font, &mut dev, &mut frame_no, e),
        };
        let load_ms = t_load.elapsed().as_millis() as u64;
        // Which pack this is: the record that came with it.
        let pack_id: Value = std::fs::read(paths::PACK_CARD_ID).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);

        let lanes = core::mem::take(&mut city.lanes);
        let mut traffic = tokyo_sim::traffic::Traffic::new(lanes.0, lanes.1, 800, 0x70_6b79);
        let mut ring = match pocket_vita_gxm::mem::Ring::new(cars::Cars::frame_bytes() + 4096, 2) {
            Ok(r) => r,
            Err(e) => fail(font, &mut dev, &mut frame_no, e),
        };
        let mut fence = Fence::new(0, 2);
        let mut scene_fence = Fence::new(2, 2);
        let control = if live { control_watcher() } else { mpsc::channel().1 };
        let mut set = Settings {
            stats: live,
            profile: false,
            show: Show { top: true, wall: true, solid: true, near: 300.0, mid: 1300.0, sectors: true, chop: 1 },
            pace: boot["pace"].as_i64().unwrap_or(1) as i32,
            look: Look::DEFAULT,
            view: None,
            hour: city.city.hour,
            rate: 0.03,
            haze: 0.00022,
            lamps: 1.0,
            tour: boot["tour"].as_bool().unwrap_or(true),
            traffic: true,
            budget: 200_000,
            govern: true,
            reach: (300.0, 1300.0),
        };
        let tour = tokyo_sim::tour::Tour::new(city.tour.clone(), 60.0);
        let mut tour_at = 0.0f32;
        let mut scale = 1.0f32;
        let home = city.city.view;
        let mut cam = Camera::looking(v3(home[0], home[1], home[2]), v3(home[3], home[4], home[5]));

        let ctx = g::vita2d_get_context();
        let mut timing = Timing { ms: [16.7; Timing::N], at: 0, late: 0, frames: 0 };
        let mut last = Instant::now();
        let mut last_vcount = sceDisplayGetVcount();
        let mut prev_buttons = u32::MAX;
        let (mut draw_ms, mut gpu_ms, mut scene_ms) = (0.0f32, 0.0f32, 0.0f32);
        let mut stats = city::Stats::default();
        let mut clock_tick = 0u32;
        let mut seconds = 0.0f32;

        loop {
            // -------------------------------------------------------------- input
            let pad = input::read();
            let (buttons, action) = dev.menu.input(pad.buttons);
            let pressed = buttons & !prev_buttons;
            prev_buttons = buttons;
            while let Ok(v) = control.try_recv() {
                apply_control(&v, &mut set, &mut cam, &mut tour_at);
            }
            let vcount = sceDisplayGetVcount();
            let vblanks = (vcount.wrapping_sub(last_vcount)).clamp(1, 4);
            last_vcount = vcount;
            let dt = vblanks as f32 / 60.0;
            seconds += dt;
            if pressed & P_SELECT != 0 && frame_no > 30 {
                set.stats = !set.stats;
            }
            if pressed & P_START != 0 && !tour.is_empty() {
                set.tour = !set.tour;
                set.view = None;
            }
            let _ = home;
            // The clock: left and right turn it by hand, up and down set how fast it runs by itself.
            if buttons & P_RIGHT != 0 {
                set.hour += 2.5 * dt;
            }
            if buttons & P_LEFT != 0 {
                set.hour -= 2.5 * dt;
            }
            if pressed & P_UP != 0 {
                set.rate = min(set.rate + 0.25, 4.0);
            }
            if pressed & P_DOWN != 0 {
                set.rate = max(set.rate - 0.25, 0.0);
            }
            set.hour = (set.hour + set.rate * dt).rem_euclid(24.0);
            let inp = if dev.menu.visible { Input::default() } else { pad_input(&pad, buttons) };
            // The sticks take the camera off the tour, where it is.
            if set.tour && (abs(inp.lx) + abs(inp.ly) + abs(inp.rx) + abs(inp.ry) > 0.2 || inp.buttons != 0) && frame_no > 30 {
                set.tour = false;
            }
            if set.tour && !tour.is_empty() {
                tour_at += dt;
                let (mut eye, target) = tour.at(tour_at);
                eye.y = max(eye.y, city.height(eye.x, eye.z) + 14.0);
                cam = Camera::looking(eye, target);
            } else {
                cam.fly(&inp, dt, |x, z| city.height(x, z));
            }

            if set.traffic {
                traffic.step(dt);
            }

            // -------------------------------------------------------------- camera and light
            let (eye, look, fov) = match set.view {
                Some((pos, target, fov)) => (pos, (target - pos).norm_or(v3(0.0, 0.0, -1.0)), fov),
                None => (cam.pos, cam.look(), cam.fov),
            };
            let aspect = 960.0 / 544.0;
            let vp = mat::mul(&mat::perspective(fov, aspect, 2.0, 12000.0), &mat::view(eye, look, 0.0));
            let light = sky::light(set.hour, DAY);
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
            if set.profile {
                // Into the scene's own target, timed by itself, then copied to the display.
                if let Err(e) = post.begin_scene(ctx) {
                    pocketjs_vita::vita_log(format_args!("tokyo: {e}"));
                }
                post.draw_sky(ctx, &sky_mvp, &sky_f);
                stats = city.draw(ctx, &vp, eye, &frame, &look_f, shadows.texture(), &set.show);
                if set.traffic {
                    cars.draw(ctx, &traffic, &mut ring, &vp, eye, &world_map, &frame, &look_f, shadows.texture());
                }
                post.end_scene(ctx, Some(scene_fence.signal(slot)));
                let tg = Instant::now();
                scene_fence.wait(slot);
                scene_ms = scene_ms * 0.9 + tg.elapsed().as_secs_f32() * 100.0;
                g::vita2d_pool_reset();
                g::vita2d_start_drawing_advanced(core::ptr::null_mut(), 0);
                post.copy_scene(ctx);
            } else {
                g::vita2d_pool_reset();
                g::vita2d_start_drawing_advanced(core::ptr::null_mut(), 0);
                post.full_view(ctx);
                post.draw_sky(ctx, &sky_mvp, &sky_f);
                stats = city.draw(ctx, &vp, eye, &frame, &look_f, shadows.texture(), &set.show);
                if set.traffic {
                    cars.draw(ctx, &traffic, &mut ring, &vp, eye, &world_map, &frame, &look_f, shadows.texture());
                }
            }
            post.add_glow(ctx, &look_now);
            // vita2d's overlay (text, the debug menu) expects its own viewport and no depth.
            g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.5, 0.5);
            gpu::state_overlay(ctx, false);
            if set.stats {
                let line = format!(
                    "{:.1} fps  {:.1} ms (worst {:.1})  late {}  cpu {:.1}  scene {:.1}  {}k tris {} draws  {}/{}/{}  {:02}:{:02}",
                    1000.0 / timing.avg().max(0.1),
                    timing.avg(),
                    timing.worst(),
                    timing.late,
                    draw_ms,
                    scene_ms,
                    (stats.tris[0] + stats.tris[1] + stats.tris[2]) / 1000,
                    stats.draws,
                    stats.places[0],
                    stats.places[1],
                    stats.places[2],
                    set.hour as u32,
                    (set.hour.fract() * 60.0) as u32,
                );
                text(font, 12, 24, 0xffff_ffff, 0.8, &line);
            }
            if !set.stats {
                let name = meta["name"].as_str().unwrap_or("Tokyo");
                text(font, 28, 520, 0xe0ff_ffff, 1.0, &format!("{name}   {:02}:{:02}{}", set.hour as u32, (set.hour.fract() * 60.0) as u32, if set.tour { "   TOUR" } else { "" }));
                if seconds < 9.0 {
                    text(font, 28, 492, 0xb0ff_ffff, 0.8, "sticks: fly and look    L R: down, up    X: fast    left right: the clock    START: tour");
                }
            }
            dev.overlay();
            g::sceGxmEndScene(ctx, core::ptr::null(), fence.signal(slot));
            draw_ms = draw_ms * 0.9 + t2.elapsed().as_secs_f32() * 100.0;
            if set.profile {
                let tg = Instant::now();
                fence.wait(slot);
                gpu_ms = gpu_ms * 0.9 + tg.elapsed().as_secs_f32() * 100.0;
            }
            // The distances of the levels of detail follow the triangle budget: in quickly when a frame draws too
            // many, out slowly when there is room.
            if set.govern {
                let drawn = stats.tris[0] + stats.tris[1] + stats.tris[2];
                if drawn > set.budget {
                    scale *= 0.97;
                } else if (drawn as f32) < set.budget as f32 * 0.88 {
                    scale *= 1.008;
                }
                scale = clamp(scale, 0.25, 1.0);
                set.show.near = set.reach.0 * scale;
                set.show.mid = set.reach.1 * scale;
            }
            g::vita2d_swap_buffers();
            // Hold the pace: a frame is shown for `pace` refreshes.
            while sceDisplayGetVcount().wrapping_sub(last_vcount) < set.pace {
                sceDisplayWaitVblankStart();
            }

            let now = Instant::now();
            let shown = sceDisplayGetVcount().wrapping_sub(last_vcount);
            timing.push((now - last).as_secs_f32() * 1000.0, shown > set.pace);
            last = now;

            // -------------------------------------------------------------- status
            clock_tick += 1;
            if clock_tick % 60 == 0 {
                // The system lowers the clocks after a suspend; set them again.
                if scePowerGetArmClockFrequency() < CLOCKS[0] - 20 {
                    set_clocks();
                }
            }
            if frame_no % 10 == 0 {
                dev.engine = json!({
                    "stage": "running",
                    "pack": {"path": pack_path, "sha256": pack_id["sha256"], "bytes": pack_id["bytes"], "name": meta["name"], "area": meta["area"], "profile": meta["profile"], "source": meta["source"]},
                    "loadMs": load_ms, "copyMs": copy_ms,
                    "frameMs": timing.avg(), "worstMs": timing.worst(), "late": timing.late, "frames": timing.frames, "pace": set.pace,
                    "cpuMs": {"draw": draw_ms},
                    "gpuMs": if set.profile { json!(gpu_ms) } else { Value::Null },
                    "sceneMs": if set.profile { json!(scene_ms) } else { Value::Null },
                    "city": {"draws": stats.draws, "tris": {"top": stats.tris[0], "wall": stats.tris[1], "solid": stats.tris[2]}, "places": stats.places, "turned": stats.turned},
                    "camera": {"pos": [eye.x, eye.y, eye.z], "look": [look.x, look.y, look.z], "fov": fov},
                    "clock": {"hour": set.hour, "rate": set.rate, "night": light.night},
                    "shadows": {"sweeps": shadows.sweeps, "sweepMs": shadows.sweep_ms()},
                    "cars": {"all": traffic.cars.len(), "shown": cars.shown},
                    "tour": {"on": set.tour, "at": tour_at, "seconds": tour.seconds()}, "governor": {"on": set.govern, "budget": set.budget, "scale": scale},
                    "settings": {"near": set.show.near, "mid": set.show.mid, "top": set.show.top, "wall": set.show.wall, "solid": set.show.solid, "profile": set.profile, "post": {"bloom": set.look.bloom}},
                    "programs": {"compiled": gpu.compiled, "cached": gpu.cached},
                    "msaa": samples,
                    "memory": {"geometry": city.geometry_bytes, "textures": city.texture_bytes, "vram": vram.reserved()},
                    "clockMhz": [scePowerGetArmClockFrequency(), scePowerGetGpuClockFrequency()],
                });
            }
            dev.publish(frame_no, "tokyo");
            serve(&mut dev, frame_no, action);
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
                request.finish(Err("Pocket Tokyo has no JS guest; use native".into()));
            }
        }
        None => {}
    }
}
