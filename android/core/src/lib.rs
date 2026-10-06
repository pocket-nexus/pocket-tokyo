//! Pocket Tokyo on Android, measured on the Redmi 1S, behind the C interface of
//! the shell (`../../src/core.h` declares the same functions).
//!
//! The shell owns the window, the EGL context, the touches and the PocketJS
//! guest. This library owns the city: the pack, the OpenGL ES 3.0 programs and
//! buffers, what a frame draws, the shadows' sweep, the traffic, the flight
//! and the flow around it (`tokyo_interface::Session`), and the interface's
//! channel, which `tokyo_interface::wire` answers in the process.
//!
//! A frame: `tk_step` (what the interface asked, one tick of the flight per
//! refresh), the guest's turn in the shell, `tk_draw` (the city, then the sky)
//! and the shell's draw of the interface over it.

mod cars;
mod city;
mod gl;
mod shadow;

use core::ffi::c_char;
use std::fmt::Write;

use city::{CityGpu, Loader, Show, Stats};
use tokyo_interface::{channel, Mode, Pace, Session};
use tokyo_sim::flight::Flight;
use tokyo_sim::mat;
use tokyo_sim::math::*;
use tokyo_sim::sky;
use tokyo_sim::traffic::Traffic;

/// The day of the year the sun follows (5 October).
const DAY: f32 = 277.0;
/// Triangles a frame may draw at most (`budget=` in a control message changes it). Under it the governor
/// follows the GPU's own time for a frame.
const BUDGET: u32 = 160_000;
/// Milliseconds of the GPU a frame, by the driver's timer, above which the levels of detail come in and
/// below which they go out again. The timer leaves out 2.4 ms of a frame (the kernel counts the GPU busy
/// 91 % of the time when the timer reads 12.7 ms), so a refresh of 16.7 ms is 14.2 ms of it.
const GPU_MS: (f32, f32) = (11.0, 13.0);
/// How far the city's lights are on (`Light::night`) from where the night's programs draw: the sun's light
/// is out by then and the moon's has begun, so the shadows leave unseen.
pub const NIGHT: f32 = 0.999;
/// The near and the mid distance when the budget allows them in full.
const REACH: (f32, f32) = (300.0, 1300.0);
/// Frames between two lines of the numbers in flight: 20 lines a second.
const NUMBERS_EVERY: u32 = 3;

const SKY_V: &str = include_str!("../../shaders/sky.vert");

/// What the shell measured of the frames it has shown.
#[repr(C)]
pub struct Perf {
    /// Milliseconds from one shown frame to the next: the mean and the worst of the last 240, and of
    /// the last one.
    frame: f32,
    worst: f32,
    last: f32,
    /// Frames shown after more refreshes than the pace asks for, and frames shown.
    late: u32,
    frames: u32,
    /// Milliseconds of this thread in a frame: the flight, the guest's turn, the city's draws, the
    /// interface's draws, and the call that shows the frame.
    step: f32,
    guest: f32,
    draw: f32,
    interface: f32,
    swap: f32,
    /// Milliseconds the GPU took over a frame, smoothed; 0 where the driver does not say.
    gpu: f32,
    gpu_now: f32,
}

struct Sky {
    program: u32,
    mvp: i32,
    sky: i32,
    array: u32,
}

struct App {
    city: CityGpu,
    cars: cars::Cars,
    shadows: shadow::Shadows,
    sky: Sky,
    traffic: Traffic,
    flight: Flight,
    session: Session,
    /// What the interface asked to have stored, until the shell takes it.
    prefs: Option<String>,
    show: Show,
    stats: Stats,
    light: sky::Light,
    seconds: f32,
    /// The picture: its lower left corner in the window's buffer, and its size, in pixels.
    at: (i32, i32),
    size: (u32, u32),
    /// The governor: how far the distances of the levels of detail are in, and frames they stay in
    /// after a late frame.
    scale: f32,
    hold: u32,
    haze: f32,
    lamps: f32,
    /// How far the lamps' light lies on the ground, 0 to 1.
    pools: f32,
    /// The GPU's milliseconds a frame the governor keeps between (`band=A,B` moves them).
    band: (f32, f32),
    late_seen: u32,
    /// The draw's parts in milliseconds, smoothed.
    ms: [f32; 2],
}

static mut LOADER: Option<Loader> = None;
static mut APP: Option<App> = None;
static mut VIEW: (i32, i32, u32, u32) = (0, 0, 1280, 720);

fn started() -> Option<&'static mut App> {
    unsafe { (*core::ptr::addr_of_mut!(APP)).as_mut() }
}

unsafe fn text<'a>(at: *const c_char, len: u32) -> &'a str {
    core::str::from_utf8(core::slice::from_raw_parts(at as *const u8, len as usize)).unwrap_or("")
}

unsafe fn put(out: *mut c_char, cap: u32, s: &str) -> u32 {
    if cap == 0 {
        return 0;
    }
    let n = s.len().min(cap as usize - 1);
    core::ptr::copy_nonoverlapping(s.as_ptr(), out as *mut u8, n);
    *out.add(n) = 0;
    n as u32
}

/// A frame of the Pocket3D title card: `tick`'s frame as RGBA rows into `pixels`. Returns 0 when the
/// card is over, 2 when the frame is the one drawn at tick `shown` (nothing is written), 1 when it drew.
///
/// # Safety
/// `pixels` has room for `width * height * 4` bytes.
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

/// What the interface shows while there is no flight: 0 the pack is being read (`message` names the
/// step), 1 the start failed (`message` says why).
///
/// # Safety
/// `message` points at `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn tk_stage(stage: u32, message: *const c_char, len: u32) {
    let state = &mut channel().state;
    state.mode = if stage == 0 { Mode::Loading } else { Mode::Error };
    state.message.clear();
    state.message.push_str(text(message, len));
}

/// The preferences the shell read from its storage at the start, for the interface.
///
/// # Safety
/// `at` points at `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn tk_prefs_stored(at: *const c_char, len: u32) {
    let state = &mut channel().state;
    state.prefs.clear();
    state.prefs.push_str(text(at, len));
}

/// What the interface asked to have stored since the last call: its length, or 0.
///
/// # Safety
/// `out` has room for `cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn tk_prefs_take(out: *mut c_char, cap: u32) -> u32 {
    let Some(a) = started() else { return 0 };
    let Some(kept) = a.prefs.take() else { return 0 };
    if kept.len() >= cap as usize {
        a.prefs = Some(kept);
        return 0;
    }
    put(out, cap, &kept)
}

/// Whether the guest's next turn is worth taking (`tokyo_interface::Pace`). `touching`: a finger is on
/// the panel. Before the flight exists every turn is.
#[no_mangle]
pub extern "C" fn tk_guest_due(buttons: u32, touching: u32) -> u32 {
    static mut PACE: Pace = Pace::new();
    match started() {
        Some(a) => unsafe { (*core::ptr::addr_of_mut!(PACE)).due(&a.session, buttons, touching != 0) as u32 },
        None => 1,
    }
}

/// A guest holds the interface's channel.
#[no_mangle]
pub extern "C" fn tk_interface_open() -> u32 {
    unsafe { channel() }.is_open() as u32
}

/// The picture in the window's buffer, whenever the shell has made its surface or the window has changed:
/// the lower left corner and the size of the rectangle the city is drawn into, in pixels. On a 1280 × 720
/// window it is the whole buffer; in a window of another shape the shell gives a 16:9 rectangle about the
/// middle.
#[no_mangle]
pub extern "C" fn tk_window(x: i32, y: i32, width: u32, height: u32) {
    unsafe { VIEW = (x, y, width, height) };
    if let Some(a) = started() {
        a.at = (x, y);
        a.size = (width, height);
    }
}

/// One step of reading the pack: a few megabytes, or a picture. `fd`, `offset`, `length`: where the
/// pack is (a file of its own, or a stored entry of the package). Returns 0 while there is more (and
/// `message` is what the loading screen says), 1 when the city is ready, -1 when it failed (and
/// `message` says why).
///
/// # Safety
/// The context every later call uses is current; `message` has room for `cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn tk_load(fd: i32, offset: i64, length: i64, message: *mut c_char, cap: u32) -> i32 {
    let loader = &mut *core::ptr::addr_of_mut!(LOADER);
    if loader.is_none() {
        match Loader::new(fd, offset, length, 1024) {
            Ok(l) => *loader = Some(l),
            Err(e) => {
                put(message, cap, &e);
                return -1;
            }
        }
    }
    match loader.as_mut().unwrap().step() {
        Ok(Some(line)) => {
            put(message, cap, &line);
            0
        }
        Ok(None) => match start(loader.take().unwrap().finish()) {
            Ok(()) => 1,
            Err(e) => {
                put(message, cap, &e);
                -1
            }
        },
        Err(e) => {
            *loader = None;
            put(message, cap, &e);
            -1
        }
    }
}

unsafe fn start(mut city: CityGpu) -> Result<(), String> {
    let c = city.city;
    let light = sky::light(c.hour, DAY);
    let shadows = shadow::Shadows::new(city.heights.clone(), c.grid_w as usize, c.grid_h as usize, c.grid_step, c.height_step, light.dir)?;
    let cars = cars::Cars::new(&city::scene_defines(&c))?;
    // The sky: a dome of unit directions around the eye.
    let program = gl::program("sky", "", SKY_V, city::SOLID_F, &["aPosition"])?;
    let dome: Vec<[f32; 3]> = (0..sky::DOME_VERTS).map(|i| sky::dome_dir(i)).map(|d| [d.x, d.y, d.z]).collect();
    let mut indices = [0u16; sky::DOME_INDICES];
    sky::dome_indices(&mut indices);
    let (mut array, mut buffers) = (0, [0u32; 2]);
    gl::glGenVertexArrays(1, &mut array);
    gl::glGenBuffers(2, buffers.as_mut_ptr());
    gl::glBindVertexArray(array);
    gl::glBindBuffer(gl::ARRAY_BUFFER, buffers[0]);
    gl::glBufferData(gl::ARRAY_BUFFER, (dome.len() * 12) as isize, dome.as_ptr().cast(), gl::STATIC_DRAW);
    gl::glBindBuffer(gl::ELEMENT_ARRAY_BUFFER, buffers[1]);
    gl::glBufferData(gl::ELEMENT_ARRAY_BUFFER, (indices.len() * 2) as isize, indices.as_ptr().cast(), gl::STATIC_DRAW);
    gl::glEnableVertexAttribArray(0);
    gl::glVertexAttribPointer(0, 3, gl::FLOAT, 0, 12, core::ptr::null());
    gl::glBindVertexArray(0);
    let sky = Sky { program, mvp: gl::uniform(program, "uMvp"), sky: gl::uniform(program, "uSky"), array };

    let lanes = core::mem::take(&mut city.lanes);
    let traffic = Traffic::new(lanes.0, lanes.1, 800, 0x70_6b79);
    // The eye, the tour and the clock, as on every device. The distances of the levels of detail stay
    // with this device's own governor (`tk_draw`), which also hears of late frames.
    let mut flight = Flight::new(c.view, c.hour, city.tour.clone(), BUDGET, REACH, 1);
    flight.governor.on = true;
    // The flow: the title over the tour, the flight, the menu. This device draws traffic, so the
    // interface may turn it off.
    let mut session = Session::new();
    session.traffic = true;
    session.numbers_every = NUMBERS_EVERY;
    channel().state.message.clear();
    *core::ptr::addr_of_mut!(APP) = Some(App {
        city,
        cars,
        shadows,
        sky,
        traffic,
        flight,
        session,
        prefs: None,
        show: Show { top: true, wall: true, solid: true, unordered: false },
        stats: Stats::default(),
        light,
        seconds: 0.0,
        at: (VIEW.0, VIEW.1),
        size: (VIEW.2, VIEW.3),
        scale: 1.0,
        hold: 0,
        haze: 0.00022,
        lamps: 1.0,
        pools: 0.0,
        band: GPU_MS,
        late_seen: 0,
        ms: [0.0; 2],
    });
    Ok(())
}

/// What the interface asked for since the last frame, then `ticks` refreshes of the flight and the
/// traffic, and what the interface is shown of it.
#[no_mangle]
pub extern "C" fn tk_step(ticks: u32) {
    let Some(a) = started() else { return };
    if let Some(kept) = a.session.obey(&mut a.flight, |_, _| {}) {
        unsafe { channel().state.prefs = kept.clone() };
        a.prefs = Some(kept);
    }
    let dt = ticks as f32 / 60.0;
    a.seconds += dt;
    let (c, heights) = (a.city.city, &a.city.heights);
    // The pad is empty: the stick, the keys and the drags are the interface's commands.
    a.session.run(&mut a.flight, &tokyo_interface::Pad::default(), dt, |x, z| city::height(&c, heights, x, z));
    if a.flight.traffic {
        a.traffic.step(dt);
    }
    a.light = sky::light(a.flight.hour, DAY);
    a.session.publish(&a.flight, unsafe { &mut channel().state });
}

/// The frame in numbers for the interface, while its statistics setting is on: twice a second.
///
/// # Safety
/// `perf` points at the shell's record.
#[no_mangle]
pub unsafe extern "C" fn tk_report(perf: *const Perf) {
    let Some(a) = started() else { return };
    let p = &*perf;
    let line = &mut channel().state.stats;
    if !a.flight.stats {
        line.clear();
    } else if p.frames % 30 == 0 || line.is_empty() {
        line.clear();
        let tris = a.stats.tris.iter().sum::<u32>();
        let _ = write!(line, "{:.1} fps · {:.1} ms · late {} · {} draws · {}k tris", 1000.0 / max(p.frame, 0.1), p.frame, p.late, a.stats.draws, tris / 1000);
    }
}

/// Draws the city into the bound target, which the shell has cleared. `perf`: what the shell measured up
/// to the frame before; the governor reads how many frames were late from it.
///
/// # Safety
/// The context of `tk_load` is current.
#[no_mangle]
pub unsafe extern "C" fn tk_draw(perf: *const Perf) {
    let Some(a) = started() else { return };
    let p = &*perf;
    let (eye, look, fov) = a.flight.eye();
    let aspect = a.size.0 as f32 / a.size.1 as f32;
    let view = mat::view(eye, look, 0.0);
    // The frustum's planes are read from a projection with depth from 0 to 1; the GPU takes -w to w.
    let cull = mat::mul(&mat::perspective(fov, aspect, 2.0, 12000.0), &view);
    let vp = mat::mul(&mat::perspective_gl(fov, aspect, 2.0, 12000.0), &view);
    let light = a.light;
    // By night nothing reads the shadows (`NIGHT`): the sweeps rest until the lights start to go out.
    let moonlit = light.night >= NIGHT;
    if !moonlit {
        a.shadows.update(light.dir);
    }
    let half = |x: [f32; 3], y: [f32; 3], s: f32| [(x[0] + s * y[0]) * 0.5, (x[1] + s * y[1]) * 0.5, (x[2] + s * y[2]) * 0.5];
    let (side, rise) = (half(light.sky, light.ground, 1.0), half(light.sky, light.ground, -1.0));
    // An upward face takes the sky's light; the fragment stage is handed that colour and a number per
    // vertex for how much of it. `up`: the shares of the two parts in its brightness.
    let lum = |c: [f32; 3]| c[0] * 0.3 + c[1] * 0.59 + c[2] * 0.11;
    let (ls, lr) = (lum(side), lum(rise));
    #[rustfmt::skip]
    let frame: [f32; 24] = [
        light.dir.x, light.dir.y, light.dir.z, a.haze,
        side[0], side[1], side[2], light.night,
        rise[0], rise[1], rise[2], ls / max(ls + lr, 1e-4),
        0.94, a.seconds, lr / max(ls + lr, 1e-4), lr / max(ls, 1e-4),
        light.sun[0], light.sun[1], light.sun[2], 0.0,
        light.horizon[0], light.horizon[1], light.horizon[2], 0.0,
    ];
    // The lamps' light lies on the ground once the night's programs draw (the ground's program reads the
    // shadows or the lamps, never both), and comes on and goes out over two seconds.
    a.pools += (if moonlit { 1.0 } else { 0.0 } - a.pools) * 0.03;
    let pools = if a.pools > 0.02 { a.pools * 2.0 * a.lamps } else { 0.0 };
    let haze = [light.horizon[0], light.horizon[1], light.horizon[2], pools];
    #[rustfmt::skip]
    let lights: [city::Light; 2] = [
        [side[0] + rise[0], side[1] + rise[1], side[2] + rise[2], 0.0, light.sun[0], light.sun[1], light.sun[2], 0.0, haze[0], haze[1], haze[2], haze[3]],
        [side[0], side[1], side[2], 0.0, light.sun[0], light.sun[1], light.sun[2], 0.0, haze[0], haze[1], haze[2], haze[3]],
    ];

    // (`tiny=1`: the frame into a sixteenth of the window each way, to measure what its geometry costs
    // without its pixels)
    let tiny = a.flight.number(tokyo_sim::flight::name(b"tiny"), 0.0) != 0.0;
    gl::glViewport(a.at.0, a.at.1, a.size.0 as i32 / if tiny { 16 } else { 1 }, a.size.1 as i32 / if tiny { 16 } else { 1 });
    gl::glDisable(gl::BLEND);
    gl::glDisable(gl::SCISSOR_TEST);
    gl::glDisable(gl::STENCIL_TEST);
    gl::glDisable(gl::DITHER);
    gl::glEnable(gl::DEPTH_TEST);
    gl::glDepthFunc(gl::LEQUAL);
    gl::glDepthMask(1);
    gl::glColorMask(1, 1, 1, 1);
    gl::glFrontFace(if a.flight.number(tokyo_sim::flight::name(b"winding"), 0.0) != 0.0 { gl::CW } else { gl::CCW });
    gl::glCullFace(gl::BACK);
    a.show.top = a.flight.number(tokyo_sim::flight::name(b"top"), 1.0) != 0.0;
    a.show.wall = a.flight.number(tokyo_sim::flight::name(b"wall"), 1.0) != 0.0;
    a.show.solid = a.flight.number(tokyo_sim::flight::name(b"solid"), 1.0) != 0.0;
    a.show.unordered = a.flight.number(tokyo_sim::flight::name(b"unordered"), 0.0) != 0.0;
    if a.flight.number(tokyo_sim::flight::name(b"cull"), 1.0) == 0.0 {
        gl::glCullFace(gl::FRONT);
    }

    // (`shadow=0`: every program reads a texture of one texel in place of the shadows, for measurements)
    let shadow = if a.flight.number(tokyo_sim::flight::name(b"shadow"), 1.0) != 0.0 && !moonlit { a.shadows.texture() } else { a.shadows.none() };
    let stats = a.city.draw(&cull, &vp, eye, &frame, &lights, shadow, &a.flight.show, &a.show);
    if a.flight.traffic {
        let c = a.city.city;
        let grid = (c.grid_w as f32 * c.grid_step, c.grid_h as f32 * c.grid_step);
        let map = [1.0 / grid.0, 1.0 / grid.1, -c.grid_x0 / grid.0, -c.grid_z0 / grid.1];
        a.cars.draw(&a.traffic, &mat::planes(&cull), &vp, eye, &map, &frame, shadow);
    }
    // The sky last: it fills what nothing else covered, and no fragment of it is drawn under the city.
    let sky_mvp = mat::mul(&mat::perspective_gl(fov, aspect, 0.5, 4.0), &mat::view(V3::ZERO, look, 0.0));
    let glow = saturate(light.sun_dir.y * 6.0 + 0.6);
    #[rustfmt::skip]
    let sky_f: [f32; 16] = [
        haze[0], haze[1], haze[2], 0.0,
        light.zenith[0], light.zenith[1], light.zenith[2], 0.0,
        light.sun_dir.x, light.sun_dir.y, light.sun_dir.z, light.dark,
        (0.6 + light.sun[0]) * glow, (0.45 + light.sun[1]) * glow, (0.3 + light.sun[2]) * glow, 0.0,
    ];
    if a.flight.number(tokyo_sim::flight::name(b"sky"), 1.0) != 0.0 {
        gl::glUseProgram(a.sky.program);
        gl::glUniformMatrix4fv(a.sky.mvp, 1, 1, sky_mvp.as_ptr());
        gl::glUniform4fv(a.sky.sky, 4, sky_f.as_ptr());
        gl::glDisable(gl::CULL_FACE);
        gl::glDepthMask(0);
        gl::glBindVertexArray(a.sky.array);
        gl::glDrawElements(gl::TRIANGLES, sky::DOME_INDICES as i32, gl::UNSIGNED_SHORT, core::ptr::null());
        gl::glBindVertexArray(0);
        gl::glDepthMask(1);
    }
    a.stats = stats;
    for k in 0..2 {
        a.ms[k] += (stats.ms[k] - a.ms[k]) * 0.1;
    }

    // The distances of the levels of detail follow the GPU's time for a frame, which the driver reports
    // (the frame of four refreshes ago, and a mean over the last ten): in while the mean is over the
    // band, in faster when one frame is a millisecond over it, out while both are under it. A frame shown
    // late counts when the GPU was near the band's top: most late frames on this phone are the system's
    // (a minute of the tour had as many at a third of the distances), and those must not cost the view.
    let late = p.late != a.late_seen;
    a.late_seen = p.late;
    if a.flight.governor.on {
        let drawn = stats.tris.iter().sum::<u32>();
        let pressed = late && p.gpu_now > a.band.1 - 0.5;
        if pressed {
            a.hold = 60;
        }
        if drawn > a.flight.governor.budget || p.gpu_now > a.band.1 + 1.0 {
            a.scale *= 0.97;
        } else if p.gpu > a.band.1 || pressed {
            a.scale *= 0.99;
        } else if a.hold == 0 && p.gpu < a.band.0 && p.gpu_now < a.band.0 + 0.5 {
            a.scale *= 1.006;
        }
        a.hold = a.hold.saturating_sub(1);
        a.scale = clamp(a.scale, 0.25, 1.0);
        a.flight.governor.scale = a.scale;
        a.flight.show.near = a.flight.governor.reach.0 * a.scale;
        a.flight.show.mid = a.flight.governor.reach.1 * a.scale;
    }
}

/// Words from the development host. `mode=title|flight|menu` sets the flow outright and
/// `ui=tour|fly|menu|resume|title` asks what the interface would ask; the rest are the flight's
/// (`Flight::control`) and this device's: `haze= lamps= top= wall= solid= sky= shadow= cull= winding=
/// unordered=`.
///
/// # Safety
/// `at` points at `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn tk_control(at: *const c_char, len: u32) {
    let words = text(at, len);
    for word in words.split_ascii_whitespace() {
        match word.split_once('=') {
            Some(("mode", name)) => {
                if let (Some(mode @ (Mode::Title | Mode::Flight | Mode::Menu)), Some(a)) = (Mode::parse(name), started()) {
                    a.session.mode = mode;
                }
            }
            Some(("ui", what)) => {
                // From the title a choice starts the flight; over a flight it hands the eye over.
                let lines: &[&str] = match what {
                    "tour" => &[r#"{"type":"start","tour":true}"#, r#"{"type":"tour","on":true}"#],
                    "fly" => &[r#"{"type":"start","tour":false}"#, r#"{"type":"tour","on":false}"#],
                    "menu" => &[r#"{"type":"menu","on":true}"#],
                    "resume" => &[r#"{"type":"menu","on":false}"#],
                    "title" => &[r#"{"type":"title"}"#],
                    _ => &[],
                };
                for line in lines {
                    channel().receive(line);
                }
            }
            Some(("haze", v)) => {
                if let (Some(a), Some(x)) = (started(), tokyo_sim::flight::parse_f32(v)) {
                    a.haze = x;
                }
            }
            Some(("band", v)) => {
                if let (Some(a), Some((lo, hi))) = (started(), v.split_once(',')) {
                    if let (Some(lo), Some(hi)) = (tokyo_sim::flight::parse_f32(lo), tokyo_sim::flight::parse_f32(hi)) {
                        a.band = (lo, hi);
                    }
                }
            }
            Some(("lamps", v)) => {
                if let (Some(a), Some(x)) = (started(), tokyo_sim::flight::parse_f32(v)) {
                    a.lamps = x;
                }
            }
            _ => {}
        }
    }
    if let Some(a) = started() {
        a.flight.control(words);
    }
}

/// The back key. Over a flight it opens the menu, under the menu it closes it; returns 0 where it has
/// nothing to do (the title, the loading screen), and the shell then leaves the app.
#[no_mangle]
pub extern "C" fn tk_back() -> u32 {
    let Some(a) = started() else { return 0 };
    let line = match a.session.mode {
        Mode::Flight => r#"{"type":"menu","on":true}"#,
        Mode::Menu => r#"{"type":"menu","on":false}"#,
        _ => return 0,
    };
    unsafe { channel().receive(line) };
    1
}

/// The run as a JSON object, for the development host. `extra`: members the shell adds.
///
/// # Safety
/// `out` has room for `cap` bytes; `perf` points at the shell's record; `extra` at `extra_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn tk_status(out: *mut c_char, cap: u32, perf: *const Perf, extra: *const c_char, extra_len: u32) -> u32 {
    let p = &*perf;
    let mut s = String::with_capacity(2048);
    let _ = write!(
        s,
        "{{\"target\":\"redmi1s\",\"frames\":{},\"frameMs\":{:.2},\"worstMs\":{:.2},\"lastMs\":{:.2},\"late\":{},\"cpuMs\":{{\"step\":{:.2},\"guest\":{:.2},\"draw\":{:.2},\"interface\":{:.2},\"swap\":{:.2}}}",
        p.frames, p.frame, p.worst, p.last, p.late, p.step, p.guest, p.draw, p.interface, p.swap
    );
    if let Some(a) = started() {
        let f = &a.flight;
        let (eye, look, fov) = f.eye();
        let t = a.stats.tris;
        let _ = write!(
            s,
            ",\"stage\":\"running\",\"mode\":\"{}\",\"pack\":\"{}\",\"window\":[{},{}],\"tris\":[{},{},{},{}],\"drawn\":{},\"draws\":{},\"places\":[{},{},{}],\"turned\":{},\"changes\":{},\"drawMs\":{{\"choose\":{:.2},\"calls\":{:.2}}},\"reach\":[{:.0},{:.0}],\"governor\":{{\"on\":{},\"budget\":{},\"scale\":{:.3},\"held\":{}}},\"clock\":{{\"hour\":{:.3},\"rate\":{:.3},\"night\":{:.2}}},\"tour\":{{\"on\":{},\"at\":{:.1},\"seconds\":{:.1}}},\"eye\":[{:.1},{:.1},{:.1}],\"look\":[{:.3},{:.3},{:.3}],\"fov\":{:.1},\"shadows\":{{\"sweeps\":{},\"sweepMs\":{:.1},\"sendMs\":{:.2}}},\"cars\":{{\"all\":{},\"shown\":{}}},\"memory\":{{\"geometry\":{},\"textures\":{}}},\"interfaceOpen\":{}",
            a.session.mode.name(),
            a.city.name,
            a.size.0,
            a.size.1,
            t[0],
            t[1],
            t[2],
            t[3],
            t.iter().sum::<u32>(),
            a.stats.draws,
            a.stats.places[0],
            a.stats.places[1],
            a.stats.places[2],
            a.stats.turned,
            a.stats.places_drawn,
            a.ms[0],
            a.ms[1],
            f.show.near,
            f.show.mid,
            f.governor.on,
            f.governor.budget,
            a.scale,
            a.hold,
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
            fov,
            a.shadows.sweeps,
            a.shadows.sweep_ms(),
            a.shadows.send_ms,
            a.traffic.cars.len(),
            a.cars.shown,
            a.city.geometry_bytes,
            a.city.texture_bytes,
            channel().is_open()
        );
    } else {
        let state = &channel().state;
        let _ = write!(s, ",\"stage\":\"{}\",\"message\":\"{}\"", if state.mode == Mode::Error { "error" } else { "loading" }, state.message.replace(['"', '\\'], " "));
    }
    let extra = text(extra, extra_len);
    if !extra.is_empty() {
        s.push(',');
        s.push_str(extra);
    }
    s.push('}');
    put(out, cap, &s)
}
