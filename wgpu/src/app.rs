//! The shell around the renderer: a frame's steps, in the order the iPod
//! touch's shell takes them (`ipod/src/main.c`), for a tab and for the build
//! machine alike.
//!
//! The core says where the eye is, what the light is and what a frame draws,
//! and decides which cells' near levels the slots hold. Where the iPod touch
//! has a thread that reads a wanted cell's record from the pack's file, this
//! shell starts a ranged read of it (`pocket_web_wgpu::source`) and hands the
//! record to the GPU on the frame after it arrives. The blocks' pictures of
//! the ground are read the same way, after the first frame, the nearest block
//! first. The shadows are swept by a [`Sweeper`]: in the frame on the build
//! machine, beside the frames in a tab.
//!
//! The screen is not one machine's: its size, its samples, the triangles a
//! frame may draw, the frames a second and what its buttons do are a
//! [`Shape`], and change while the city flies.
//!
//! The interface is the PocketJS guest of `ui/`, run by the page. A frame is
//! two calls with the guest's turn between them, as on a device: [`App::step`]
//! (what the interface asked for, then the flight), the turn (the page hands
//! it [`App::heard`] and brings back what it says to [`App::say`]), and
//! [`App::draw`], which lays the interface's picture over the scene. The
//! shell is up before the city is: until [`App::fly`] the frames show the
//! interface alone, which says what is being read.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::overlay::Overlay;
use pocket_web_wgpu::source::Source;
use pocket_web_wgpu::task;
use tokyo_core::{Pad, Perf, View, SLOTS};
use tokyo_pack::KINDS;
use tokyo_sim::view::Item;

use crate::pack::{self, Tables, MAP_SIDE};
use crate::render::{Renderer, Stats};

/// `tokyo_core`'s states of a slot.
const WANTED: u32 = 1;
const READY: u32 = 2;
/// Reads of cells under way at once: the nearest wanted cells first.
const READS: u32 = 4;
/// Cells handed to the GPU in one frame.
const ARRIVALS: usize = 2;
/// Reads of blocks' pictures under way at once.
const PICTURES: u32 = 2;
/// The ground of the Pocket3D title card: what a frame shows under the interface while there is no city.
const GROUND: [f32; 3] = [0.09, 0.07, 0.15];
/// Frames whose intervals the worst is taken over.
const WINDOW: usize = 120;

/// PocketJS's button bits (`contracts/spec`): what a page holds of a handheld's buttons, and what the
/// interface's guest is handed.
pub mod button {
    pub const SELECT: u32 = 0x0001;
    pub const START: u32 = 0x0008;
    pub const RIGHT: u32 = 0x0020;
    pub const LEFT: u32 = 0x0080;
    pub const L: u32 = 0x0100;
    pub const R: u32 = 0x0200;
    /// The face buttons by their place: the PlayStation's marks, and the 3DS's X, A, B and Y.
    pub const TOP: u32 = 0x1000;
    pub const BOTTOM: u32 = 0x4000;
    pub const LEFT_FACE: u32 = 0x8000;
}

/// What a page holds of a handheld's controls: its buttons, and its sticks in -1…1, right and up positive.
#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    pub buttons: u32,
    pub left: [f32; 2],
    pub right: [f32; 2],
}

/// A screen and the frame it asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    /// Samples a pixel of the scene is drawn with.
    pub samples: u32,
    /// Triangles a frame may draw: the distances of the levels of detail follow it.
    pub budget: u32,
    /// Frames a second.
    pub hz: u32,
    /// The interface's turns a second: what the device's own host tells its guest (`globalThis.__simHz`).
    pub turns: u32,
}

/// The screens of the handhelds this city runs on, with the frame each one's own build asks for
/// (`BUDGET` in `psp/src/main.rs`, `vita/src/main.rs`, `n3ds/src/main.c` and `ipod/src/main.c`) and the
/// turns a second its host gives the interface (`TURNS` in `psp/src/interface.rs`, `RATE` in
/// `vita/src/interface.rs`, `rate` in `n3ds/src/guest.c`, `TURN_HZ` in `ipod/src/main.c`).
/// Whatever the shape, the pack is the one that was opened: a shape changes how much of it a frame draws.
pub const SHAPES: [Shape; 4] = [
    Shape { name: "psp", width: 480, height: 272, samples: 4, budget: 42_000, hz: 30, turns: 30 },
    Shape { name: "vita", width: 960, height: 544, samples: 4, budget: 200_000, hz: 60, turns: 30 },
    Shape { name: "3ds", width: 400, height: 240, samples: 4, budget: 60_000, hz: 30, turns: 30 },
    Shape { name: "ipod", width: 480, height: 320, samples: 4, budget: 24_000, hz: 60, turns: 60 },
];

impl Shape {
    pub fn named(name: &str) -> Option<Shape> {
        SHAPES.iter().copied().find(|s| s.name == name)
    }

    /// Sixtieths of a second in one turn of the interface.
    pub fn turn(&self) -> u32 {
        60 / self.turns.clamp(1, 60)
    }

    /// Display refreshes a frame is shown for, at 60 a second.
    pub fn pace(&self) -> u32 {
        (60 / self.hz.max(1)).clamp(1, 6)
    }

    /// The flight's pad from this handheld's controls, as its own build reads them (`read_pad` in
    /// `psp/src/main.rs` and `n3ds/src/main.c`, `session_pad` in `vita/src/main.rs`). The iPod touch has no
    /// pad: its stick and keys are the interface's commands.
    pub fn pad(&self, held: &Held) -> Pad {
        use tokyo_interface::pad;
        let on = |bit: u32, to: u32| if held.buttons & bit != 0 { to } else { 0 };
        let keys = on(button::SELECT, pad::TOUR) | on(button::RIGHT, pad::LATER) | on(button::LEFT, pad::EARLIER) | on(button::START, pad::MENU);
        let climb = on(button::R, pad::UP) | on(button::L, pad::DOWN);
        // One stick flies ahead and turns; two buttons look up and down.
        let pitch = ((held.buttons & button::TOP != 0) as i32 - (held.buttons & button::BOTTOM != 0) as i32) as f32 * 0.7;
        match self.name {
            "vita" => Pad { buttons: on(button::BOTTOM, pad::FAST) | climb, keys, lx: held.left[0], ly: held.left[1], rx: held.right[0], ry: held.right[1] },
            "psp" | "3ds" => Pad { buttons: on(button::LEFT_FACE, pad::FAST) | climb, keys, lx: 0.0, ly: held.left[1], rx: held.left[0], ry: pitch },
            _ => Pad { buttons: 0, keys: 0, lx: 0.0, ly: 0.0, rx: 0.0, ry: 0.0 },
        }
    }
}

/// Sweeps the shadows for a direction of the sun into `MAP_SIDE` squared texels of 8 bits.
pub trait Sweeper {
    /// Starts a sweep, when none is under way. `false`: one is; ask again.
    fn start(&mut self, sun: [f32; 3], shade: f32) -> bool;
    /// The sweep that was started, once: its texels and the milliseconds it took.
    fn finished(&mut self) -> Option<(&[u8], f32)>;
}

/// Sweeps in the frame that asks: the frame waits for it.
pub struct SweepHere {
    swept: Vec<u16>,
    texels: Vec<u8>,
    took: Option<f32>,
}

impl SweepHere {
    pub fn new(tables: &Tables) -> SweepHere {
        SweepHere { swept: vec![0; (tables.city.grid_w * tables.city.grid_h) as usize], texels: vec![255; (MAP_SIDE * MAP_SIDE) as usize], took: None }
    }

    /// One sweep, after `tk_init`.
    pub fn sweep(&mut self, sun: [f32; 3], shade: f32) -> &[u8] {
        unsafe { tokyo_core::tk_shadows(sun.as_ptr(), shade, self.swept.as_mut_ptr(), self.texels.as_mut_ptr(), MAP_SIDE) };
        &self.texels
    }
}

impl Sweeper for SweepHere {
    fn start(&mut self, sun: [f32; 3], shade: f32) -> bool {
        let from = task::now();
        self.sweep(sun, shade);
        self.took = Some((task::now() - from) as f32);
        true
    }

    fn finished(&mut self) -> Option<(&[u8], f32)> {
        self.took.take().map(|ms| (&self.texels[..], ms))
    }
}

type Arrival = (usize, u32, Result<Vec<u8>, String>);
type Arrivals<T> = Rc<RefCell<VecDeque<T>>>;

/// The city once the pack's head has been read: what a flight needs, made beside the frames.
pub struct City {
    renderer: Renderer,
    tables: &'static Tables,
    source: Source,
}

impl City {
    /// Reads the pack's head from `source` and hands what stays in memory to the GPU, for a screen of this
    /// format and this many samples a pixel.
    pub async fn read(gpu: Gpu, format: pocket_web_wgpu::wgpu::TextureFormat, samples: u32, source: Source) -> Result<City, String> {
        let (tables, sections) = pack::open(&source).await?;
        // The core keeps the tables' addresses for the life of the program.
        let tables: &'static Tables = Box::leak(Box::new(tables));
        let renderer = Renderer::new(&gpu, format, samples, tables, &sections.stays()?)?;
        Ok(City { renderer, tables, source })
    }
}

/// The flight and what it reads.
struct Flying {
    renderer: Renderer,
    tables: &'static Tables,
    source: Source,
    /// Records that were read, until a frame hands them to the GPU.
    arrived: Arrivals<Arrival>,
    /// The cell each slot's read was started for.
    asked: [u32; SLOTS],
    reading: Rc<Cell<u32>>,
    /// After a read that failed, the time before which no other is started.
    wait_until: f64,
    /// The blocks whose pictures have been asked for, the pictures that were read, and how many the GPU has.
    pictured: Vec<bool>,
    pictures: Arrivals<(usize, Result<Vec<u8>, String>)>,
    picturing: Rc<Cell<u32>>,
    pictures_here: u32,
    sweeper: Box<dyn Sweeper>,
    /// The sun and the shade of the sweep that was last started.
    swept_for: ([f32; 3], f32),
    /// The frame the last step made, for the draw that follows it.
    view: View,
    cells_read: u32,
    sweeps: u32,
    sweep_ms: f32,
}

pub struct App {
    pub gpu: Gpu,
    pub screen: Screen,
    pub shape: Shape,
    /// The interface's picture, laid over every frame.
    pub overlay: Overlay,
    flying: Option<Flying>,
    pub perf: Perf,
    pub stats: Stats,
    intervals: [f32; WINDOW],
    last: Option<f64>,
    /// Ticks of the flight that time has passed for and no frame has taken yet.
    owed: f32,
    /// What the guest is owed: ticks no turn of its has taken, and the buttons and the contact held at any
    /// moment since a turn was last offered.
    guest: (u32, u32, bool),
    average: f32,
    /// When the frame being made began, for what it costs.
    began: f64,
    /// What last went wrong with a read or a frame, for the status.
    pub trouble: String,
}

/// The core's slots.
fn slots() -> &'static mut [tokyo_core::Slot] {
    // One thread runs the flight and reads the slots.
    unsafe { core::slice::from_raw_parts_mut(tokyo_core::tk_slots(), SLOTS) }
}

impl App {
    /// The shell on a screen, with no city yet: its frames show the interface alone. One to a program: the
    /// core keeps one flight.
    pub fn open(gpu: Gpu, screen: Screen, shape: Shape) -> App {
        let overlay = Overlay::new(&gpu, screen.format);
        let mut app = App {
            gpu,
            screen,
            shape,
            overlay,
            flying: None,
            perf: Perf { frame: 0.0, worst: 0.0, late: 0, frames: 0, cpu: 0.0, gpu: 0.0, draws: 0, tris: [0; KINDS] },
            stats: Stats::default(),
            intervals: [0.0; WINDOW],
            last: None,
            owed: 0.0,
            guest: (shape.turn(), 0, false),
            average: 1000.0 / shape.hz as f32,
            began: 0.0,
            trouble: String::new(),
        };
        app.stage("Reading the city");
        app.reshape(shape);
        app
    }

    /// The city has been read: the flight starts. `sweeper` makes what sweeps its shadows.
    pub fn fly(&mut self, city: City, sweeper: impl FnOnce(&'static Tables) -> Box<dyn Sweeper>) -> Result<(), String> {
        let City { renderer, tables, source } = city;
        let refusal = unsafe { tokyo_core::tk_init(&tables.for_core(), self.shape.budget) };
        if !refusal.is_null() {
            return Err(unsafe { core::ffi::CStr::from_ptr(refusal) }.to_string_lossy().into_owned());
        }
        self.flying = Some(Flying {
            renderer,
            tables,
            source,
            arrived: Default::default(),
            asked: [u32::MAX; SLOTS],
            reading: Default::default(),
            wait_until: 0.0,
            pictured: vec![false; tables.blocks.len()],
            pictures: Default::default(),
            picturing: Default::default(),
            pictures_here: 0,
            sweeper: sweeper(tables),
            swept_for: ([0.0; 3], -1.0),
            view: View::default(),
            cells_read: 0,
            sweeps: 0,
            sweep_ms: 0.0,
        });
        self.reshape(self.shape);
        Ok(())
    }

    /// Whether the city flies.
    pub fn flies(&self) -> bool {
        self.flying.is_some()
    }

    /// Another screen from the next frame on: its size, its samples, its budget and its frames a second.
    pub fn reshape(&mut self, shape: Shape) {
        if (self.screen.width, self.screen.height, self.screen.samples) != (shape.width, shape.height, shape.samples) {
            self.screen.resize(&self.gpu, shape.width, shape.height, shape.samples);
        }
        self.shape = shape;
        // (twelve lines of the numbers in flight a second, as on the iPod touch)
        tokyo_core::tk_shape(shape.width as f32 / shape.height as f32, (shape.hz / 12).max(1));
        if self.flying.is_some() {
            self.control(&format!("budget={} pace={}", shape.budget, shape.pace()));
        }
    }

    /// Words for the flow (`mode=`, `ui=`) and for the flight (`tokyo_sim::flight::Flight::control`).
    pub fn control(&mut self, words: &str) {
        unsafe {
            tokyo_core::tk_remote(words.as_ptr(), words.len() as u32);
            if self.flying.is_some() {
                tokyo_core::tk_control(words.as_ptr(), words.len() as u32);
            }
        }
    }

    // ---- the interface: a PocketJS guest the page runs, heard and answered through the core's channel

    /// What the interface shows while there is no flight: the step of the reading.
    pub fn stage(&mut self, step: &str) {
        unsafe { tokyo_core::tk_stage(0, step.as_ptr(), step.len() as u32) };
    }

    /// The start failed: the interface says why.
    pub fn fail(&mut self, why: &str) {
        self.trouble = why.into();
        unsafe { tokyo_core::tk_stage(1, why.as_ptr(), why.len() as u32) };
    }

    /// A guest has opened the interface's channel: it is told the whole state on its next turn. A guest that
    /// replaces another (another screen's presentation) opens it again.
    pub fn interface_opened(&mut self) {
        unsafe { tokyo_interface::channel() }.open("pocket.overlay");
        // (its first turn is this frame's, and nothing held before it was there is its to hear)
        self.guest = (self.shape.turn(), 0, false);
    }

    /// The guest's turn in this frame, once a frame after [`App::step`]: the sixtieths of a second it is for
    /// and the buttons it is handed (PocketJS's bits, held at any moment since a turn was last offered), or
    /// `None` for a frame without one. `touching`: a contact is on a surface the guest draws, or has just
    /// left.
    ///
    /// A turn is offered `shape.turns` times a second, as the device's own host offers it
    /// (`Ui::turn` in `vita/src/interface.rs`), and is always that long; it is taken when it is worth its
    /// cost (`tokyo_interface::Pace`). No more than two are owed: a guest that fell behind does not run
    /// after the time.
    pub fn guest_due(&mut self, touching: bool) -> Option<(u32, u32)> {
        let turn = self.shape.turn();
        let (owed, buttons, touched) = &mut self.guest;
        *touched |= touching;
        *owed = (*owed).min(2 * turn);
        if *owed < turn {
            return None;
        }
        *owed -= turn;
        let (buttons, touched) = (core::mem::take(buttons), core::mem::take(touched));
        (tokyo_core::tk_guest_due(buttons, touched as u32) != 0).then_some((turn, buttons))
    }

    /// The line of state the guest has not seen, for its turn (`ui/app/protocol.ts`).
    pub fn heard(&mut self) -> Option<String> {
        unsafe { tokyo_interface::channel() }.poll()
    }

    /// A line the guest sent: a command for the next step.
    pub fn say(&mut self, line: &str) {
        unsafe { tokyo_interface::channel() }.receive(line);
    }

    /// The settings the page kept from the last visit, for the interface.
    pub fn prefs_stored(&mut self, text: &str) {
        unsafe { tokyo_core::tk_prefs_stored(text.as_ptr(), text.len() as u32) };
    }

    /// What the interface asked to have kept since the last call.
    pub fn prefs_take(&mut self) -> Option<String> {
        let mut out = vec![0u8; 4096];
        let n = unsafe { tokyo_core::tk_prefs_take(out.as_mut_ptr(), out.len() as u32) };
        out.truncate(n as usize);
        (n > 0).then(|| String::from_utf8(out).ok()).flatten()
    }

    // ---- a frame

    /// The first half of a frame at `now` (milliseconds on a clock that goes forward): what the interface
    /// asked for since the last frame, then the flight. Returns the sixtieths of a second that passed.
    pub fn step(&mut self, now: f64, held: &Held) -> u32 {
        self.began = task::now();
        let interval = self.last.map(|last| (now - last) as f32);
        self.last = Some(now);
        let ticks = self.ticks(interval.unwrap_or(1000.0 / 60.0));
        if let Some(interval) = interval {
            let p = &mut self.perf;
            self.intervals[p.frames as usize % WINDOW] = interval;
            self.average += (interval - self.average) * 0.05;
            // A frame is shown for its refreshes; one that took half a refresh more is late.
            p.late += (self.flying.is_some() && interval > (self.shape.pace() as f32 + 0.5) * 1000.0 / 60.0) as u32;
        }
        self.guest.0 += ticks;
        self.guest.1 |= held.buttons;
        let pad = self.shape.pad(held);
        if let Some(f) = &mut self.flying {
            unsafe {
                // (a frame that no tick has passed for shows the last one's flight again)
                if ticks > 0 {
                    tokyo_core::tk_step(&pad, ticks);
                }
                tokyo_core::tk_report(&self.perf);
                tokyo_core::tk_view(&mut f.view);
            }
        }
        ticks
    }

    /// The second half: what has arrived goes to the GPU, the core chooses the draws, and the scene is drawn
    /// with the interface over it. Before the city flies, the interface alone over the title card's ground.
    pub fn draw(&mut self) -> Result<(), String> {
        let now = self.last.unwrap_or(0.0);
        let Some(f) = &mut self.flying else {
            let target = self.screen.frame(&self.gpu)?;
            let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
            drop(target.pass(&mut encoder, GROUND));
            self.overlay.draw(&mut encoder, &target);
            self.gpu.queue.submit([encoder.finish()]);
            target.present();
            self.perf.frames += 1;
            return Ok(());
        };
        let busy = f.arrivals(&self.gpu, now, &mut self.trouble);
        f.shadows(&self.gpu, busy);
        let mut lists: [*const Item; KINDS] = [core::ptr::null(); KINDS];
        let mut lengths = [0u32; KINDS];
        unsafe {
            tokyo_core::tk_choose(lists.as_mut_ptr(), lengths.as_mut_ptr());
            tokyo_core::tk_refill();
        }
        f.ask(now);
        f.picture(&self.gpu, &mut self.trouble);
        // (the lists are the core's until it chooses again)
        let lists: [&[Item]; KINDS] = core::array::from_fn(|k| if lengths[k] == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(lists[k], lengths[k] as usize) } });
        self.stats = f.renderer.frame(&self.gpu, &self.screen, f.tables, &f.view, lists, &self.overlay)?;
        tokyo_core::tk_drew(self.stats.tris.iter().sum());

        let p = &mut self.perf;
        p.frames += 1;
        p.frame = self.average;
        p.worst = self.intervals.iter().take(p.frames as usize).fold(0.0, |a, &b| a.max(b));
        p.cpu = (task::now() - self.began) as f32;
        p.draws = self.stats.draws;
        p.tris = self.stats.tris;
        Ok(())
    }

    /// A whole frame with no guest's turn in it.
    pub fn frame(&mut self, now: f64, held: &Held) -> Result<(), String> {
        self.step(now, held);
        self.draw()
    }

    /// Reads beside the frames while none is drawn (the title card is playing): the blocks' pictures.
    pub fn pump(&mut self) {
        if let Some(f) = &mut self.flying {
            f.picture(&self.gpu, &mut self.trouble);
        }
    }

    /// Ticks of the flight (sixtieths of a second) for a frame that comes `interval` milliseconds after the
    /// last. The handhelds' displays refresh 60 times a second and a frame there is a whole number of ticks;
    /// so it is here when the time between two frames is within a twentieth of a whole number of them (a
    /// display of 59.94 a second, or of 120 with every second refresh drawn), and a late frame catches up.
    /// On any other display the part of a tick left over is owed to the next frame, so the flight keeps
    /// its speed.
    fn ticks(&mut self, interval: f32) -> u32 {
        let most = 3.max(self.shape.pace()) as f32;
        // (a clock that went back, as after a measurement made ahead of it, is one frame's time)
        let interval = if interval > 0.0 { interval } else { 1000.0 / self.shape.hz as f32 };
        let passed = interval.min(100.0) * 0.06;
        let whole = (passed + 0.5).floor();
        if whole >= 1.0 && (passed - whole).abs() <= 0.05 * whole {
            self.owed = 0.0;
            return whole.min(most) as u32;
        }
        self.owed += passed;
        let ticks = self.owed.floor().min(most);
        self.owed = (self.owed - ticks).min(1.0);
        ticks as u32
    }

    /// Whether everything the eye is near has arrived: no cell is wanted or on its way, and every block has
    /// its picture.
    pub fn settled(&self) -> bool {
        self.flying.as_ref().is_some_and(|f| f.reading.get() == 0 && f.arrived.borrow().is_empty() && f.pictures_here as usize == f.pictured.len() && slots().iter().all(|s| s.state.load(Ordering::Acquire) != WANTED))
    }

    /// The run as a JSON object: the core's status (`tk_status`) with this shell's members.
    pub fn status(&self) -> String {
        let mut extra = String::with_capacity(512);
        let mut trouble = String::new();
        for c in self.trouble.chars() {
            match c {
                '"' | '\\' => trouble.extend(['\\', c]),
                c if c < ' ' => trouble.push(' '),
                c => trouble.push(c),
            }
        }
        let s = &self.shape;
        let _ = write!(
            extra,
            "\"mode\":\"{}\",\"interface\":{},\"fps\":{:.3},\"shape\":{{\"name\":\"{}\",\"width\":{},\"height\":{},\"samples\":{},\"budget\":{},\"hz\":{},\"turns\":{}}},\"adapter\":\"{}\",\"trouble\":\"{}\"",
            // (what the screen is for, as the interface is told, and whether a guest holds its channel)
            unsafe { tokyo_interface::channel() }.state.mode.name(),
            unsafe { tokyo_interface::channel() }.is_open(),
            if self.perf.frame > 0.0 { 1000.0 / self.perf.frame } else { 0.0 },
            s.name,
            s.width,
            s.height,
            s.samples,
            s.budget,
            s.hz,
            s.turns,
            self.gpu.adapter.replace(['"', '\\'], " "),
            trouble
        );
        let Some(f) = &self.flying else { return format!("{{\"target\":\"wgpu\",\"stage\":\"loading\",\"frames\":{},{extra}}}", self.perf.frames) };
        let read = f.source.read_so_far();
        let _ = write!(
            extra,
            ",\"slotBytes\":{},\"read\":{{\"requests\":{},\"bytes\":{},\"piece\":{},\"cells\":{},\"underWay\":{},\"pictures\":{},\"blocks\":{}}},\"shadows\":{{\"sweeps\":{},\"ms\":{:.2}}},\"statistics\":{}",
            f.tables.slot_bytes,
            read.requests,
            read.bytes,
            // (0: the pack is one file, read a range at a time)
            f.source.piece().unwrap_or(0),
            f.cells_read,
            f.reading.get() + f.picturing.get(),
            f.pictures_here,
            f.pictured.len(),
            f.sweeps,
            f.sweep_ms,
            // (the interface's statistics setting, as the flight holds it)
            f.view.stats != 0
        );
        let mut out = vec![0u8; 4096];
        let n = unsafe { tokyo_core::tk_status(out.as_mut_ptr(), out.len() as u32, &self.perf, extra.as_ptr(), extra.len() as u32) };
        out.truncate(n as usize);
        String::from_utf8(out).unwrap_or_default()
    }

    /// The place's name and its sources, as the pack's `META` gives them (JSON).
    pub fn meta(&self) -> &str {
        self.flying.as_ref().and_then(|f| core::str::from_utf8(&f.tables.meta).ok()).unwrap_or("{}")
    }
}

impl Flying {
    /// What has arrived goes to the GPU, and the slots that hold it are ready.
    fn arrivals(&mut self, gpu: &Gpu, now: f64, trouble: &mut String) -> bool {
        let mut any = false;
        for _ in 0..ARRIVALS {
            let Some((slot, cell, bytes)) = self.arrived.borrow_mut().pop_front() else { break };
            self.asked[slot] = u32::MAX;
            let s = &slots()[slot];
            // (a slot keeps its cell while it is wanted)
            if s.cell != cell || s.state.load(Ordering::Acquire) != WANTED {
                continue;
            }
            match bytes.and_then(|bytes| self.renderer.cell(gpu, slot, &self.tables.near[cell as usize], &bytes)) {
                Ok(()) => {
                    s.state.store(READY, Ordering::Release);
                    self.cells_read += 1;
                    any = true;
                }
                Err(e) => {
                    // It is asked for again in a second.
                    *trouble = e;
                    self.wait_until = now + 1000.0;
                }
            }
        }
        any
    }

    /// Starts reads for the wanted cells, the nearest first.
    fn ask(&mut self, now: f64) {
        if now < self.wait_until {
            return;
        }
        while self.reading.get() < READS {
            let table = slots();
            let wanted = |i: &usize| table[*i].state.load(Ordering::Acquire) == WANTED && self.asked[*i] != table[*i].cell;
            let Some(pick) = (0..SLOTS).filter(wanted).min_by_key(|&i| table[i].rank.load(Ordering::Relaxed)) else { break };
            let (cell, offset, size) = (table[pick].cell, table[pick].offset, table[pick].size);
            self.asked[pick] = cell;
            self.reading.set(self.reading.get() + 1);
            let (source, arrived, reading) = (self.source.clone(), self.arrived.clone(), self.reading.clone());
            task::spawn(async move {
                let bytes = source.range(offset as u64, size as u64).await;
                arrived.borrow_mut().push_back((pick, cell, bytes));
                reading.set(reading.get() - 1);
            });
        }
    }

    /// The blocks' pictures of the ground: one that has arrived goes to the GPU, and the nearest blocks
    /// without one are asked for.
    fn picture(&mut self, gpu: &Gpu, trouble: &mut String) {
        if let Some((block, bytes)) = self.pictures.borrow_mut().pop_front() {
            match bytes.and_then(|bytes| self.renderer.block(gpu, self.tables, block, &bytes)) {
                Ok(()) => self.pictures_here += 1,
                Err(e) => {
                    // (it is asked for again when the others have been)
                    *trouble = e;
                    self.pictured[block] = false;
                }
            }
        }
        let eye = self.tables.city.view;
        let eye = if self.view.fov > 0.0 { (self.view.eye[0], self.view.eye[2]) } else { (eye[0], eye[2]) };
        // (at most so many are started by one call, also where a read is done as soon as it is asked for)
        for _ in 0..PICTURES {
            if self.picturing.get() >= PICTURES {
                break;
            }
            let far = |block: &usize| {
                let (x, z) = self.tables.middle(*block);
                (((x - eye.0) * (x - eye.0) + (z - eye.1) * (z - eye.1)) as u32, *block)
            };
            let Some(block) = (0..self.pictured.len()).filter(|b| !self.pictured[*b]).min_by_key(far) else { break };
            self.pictured[block] = true;
            self.picturing.set(self.picturing.get() + 1);
            let (offset, size) = self.tables.picture(block);
            let (source, pictures, picturing) = (self.source.clone(), self.pictures.clone(), self.picturing.clone());
            task::spawn(async move {
                let bytes = source.range(offset, size).await;
                pictures.borrow_mut().push_back((block, bytes));
                picturing.set(picturing.get() - 1);
            });
        }
    }

    /// The shadows follow the sun: a sweep when it has moved a degree, or the shade has changed.
    fn shadows(&mut self, gpu: &Gpu, busy: bool) {
        let (sun, shade) = (self.view.sun, self.view.shade);
        let lit = sun != [0.0; 3];
        let (was, shaded) = self.swept_for;
        // (the cosine of one degree; and the shade is written into the texture, so a change of it counts too)
        let moved = sun[0] * was[0] + sun[1] * was[1] + sun[2] * was[2] < 0.99985 || (shade - shaded).abs() > 0.03;
        // A frame that hands a cell to the GPU does not sweep as well.
        if lit && moved && !busy && self.sweeper.start(sun, shade) {
            self.swept_for = (sun, shade);
        }
        // (a sweep of this frame's own is there at once; one from beside the frames when it has arrived)
        if let Some((texels, ms)) = self.sweeper.finished() {
            self.renderer.shadows(gpu, texels);
            self.sweeps += 1;
            self.sweep_ms = ms;
        }
    }
}
