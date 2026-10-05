//! The shell around the renderer: a frame's steps, in the order the iPod
//! touch's shell takes them (`ipod/src/main.c`), for a tab and for the build
//! machine alike.
//!
//! The core says where the eye is, what the light is and what a frame draws,
//! and decides which cells' near levels the slots hold. Where the iPod touch
//! has a thread that reads a wanted cell's record from the pack's file, this
//! shell starts a ranged read of it (`pocket_web_wgpu::source`) and hands the
//! record to the GPU on the frame after it arrives. The shadows are swept by
//! a [`Sweeper`]: in the frame on the build machine, beside the frames in a
//! tab.
//!
//! The screen is not one machine's: its size, its samples, the triangles a
//! frame may draw and the frames a second are a [`Shape`], and change while
//! the city flies.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use pocket_web_wgpu::gpu::{Gpu, Screen};
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
/// Frames whose intervals the worst is taken over.
const WINDOW: usize = 120;

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
}

/// The screens of the handhelds this city runs on, with the frame each one's own build asks for
/// (`BUDGET` in `psp/src/main.rs`, `vita/src/main.rs`, `n3ds/src/main.c` and `ipod/src/main.c`).
/// Whatever the shape, the pack is the one that was opened: a shape changes how much of it a frame draws.
pub const SHAPES: [Shape; 4] = [
    Shape { name: "psp", width: 480, height: 272, samples: 4, budget: 42_000, hz: 30 },
    Shape { name: "vita", width: 960, height: 544, samples: 4, budget: 200_000, hz: 60 },
    Shape { name: "3ds", width: 400, height: 240, samples: 4, budget: 60_000, hz: 30 },
    Shape { name: "ipod", width: 480, height: 320, samples: 4, budget: 24_000, hz: 60 },
];

impl Shape {
    pub fn named(name: &str) -> Option<Shape> {
        SHAPES.iter().copied().find(|s| s.name == name)
    }

    /// Display refreshes a frame is shown for, at 60 a second.
    pub fn pace(&self) -> u32 {
        (60 / self.hz.max(1)).clamp(1, 6)
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

pub struct App {
    pub gpu: Gpu,
    pub screen: Screen,
    pub shape: Shape,
    renderer: Renderer,
    tables: &'static Tables,
    source: Source,
    /// Records that were read, until a frame hands them to the GPU.
    arrived: Rc<RefCell<VecDeque<Arrival>>>,
    /// The cell each slot's read was started for.
    asked: [u32; SLOTS],
    reading: Rc<Cell<u32>>,
    /// After a read that failed, the time before which no other is started.
    wait_until: f64,
    sweeper: Box<dyn Sweeper>,
    /// The sun and the shade of the sweep that was last started.
    swept_for: ([f32; 3], f32),
    pub perf: Perf,
    pub stats: Stats,
    intervals: [f32; WINDOW],
    last: Option<f64>,
    /// Ticks of the flight that time has passed for and no frame has taken yet.
    owed: f32,
    average: f32,
    cells_read: u32,
    sweeps: u32,
    sweep_ms: f32,
    /// What last went wrong with a read or a frame, for the status.
    pub trouble: String,
}

/// The core's slots.
fn slots() -> &'static mut [tokyo_core::Slot] {
    // One thread runs the flight and reads the slots.
    unsafe { core::slice::from_raw_parts_mut(tokyo_core::tk_slots(), SLOTS) }
}

impl App {
    /// Reads the pack's head from `source` and starts the flight. One to a program: the core keeps one flight.
    pub async fn start(gpu: Gpu, screen: Screen, shape: Shape, source: Source, sweeper: impl FnOnce(&'static Tables) -> Box<dyn Sweeper>) -> Result<App, String> {
        let (tables, sections) = pack::open(&source).await?;
        // The core keeps the tables' addresses for the life of the program.
        let tables: &'static Tables = Box::leak(Box::new(tables));
        let renderer = Renderer::new(&gpu, &screen, tables, &sections.stays()?)?;
        drop(sections);
        let refusal = unsafe { tokyo_core::tk_init(&tables.for_core(), shape.budget) };
        if !refusal.is_null() {
            return Err(unsafe { core::ffi::CStr::from_ptr(refusal) }.to_string_lossy().into_owned());
        }
        let mut app = App {
            gpu,
            screen,
            shape,
            renderer,
            tables,
            source,
            arrived: Rc::new(RefCell::new(VecDeque::new())),
            asked: [u32::MAX; SLOTS],
            reading: Rc::new(Cell::new(0)),
            wait_until: 0.0,
            sweeper: sweeper(tables),
            swept_for: ([0.0; 3], -1.0),
            perf: Perf { frame: 0.0, worst: 0.0, late: 0, frames: 0, cpu: 0.0, gpu: 0.0, draws: 0, tris: [0; KINDS] },
            stats: Stats::default(),
            intervals: [0.0; WINDOW],
            last: None,
            owed: 0.0,
            average: 1000.0 / shape.hz as f32,
            cells_read: 0,
            sweeps: 0,
            sweep_ms: 0.0,
            trouble: String::new(),
        };
        app.reshape(shape);
        Ok(app)
    }

    /// Another screen from the next frame on: its size, its samples, its budget and its frames a second.
    pub fn reshape(&mut self, shape: Shape) {
        if (self.screen.width, self.screen.height, self.screen.samples) != (shape.width, shape.height, shape.samples) {
            self.screen.resize(&self.gpu, shape.width, shape.height, shape.samples);
        }
        self.shape = shape;
        // (twelve lines of the numbers in flight a second, as on the iPod touch)
        tokyo_core::tk_shape(shape.width as f32 / shape.height as f32, (shape.hz / 12).max(1));
        self.control(&format!("budget={} pace={}", shape.budget, shape.pace()));
    }

    /// Words for the flow (`mode=`, `ui=`) and for the flight (`tokyo_sim::flight::Flight::control`).
    pub fn control(&mut self, words: &str) {
        unsafe {
            tokyo_core::tk_remote(words.as_ptr(), words.len() as u32);
            tokyo_core::tk_control(words.as_ptr(), words.len() as u32);
        }
    }

    /// A line of the interface's protocol, as its guest would send it (`ui/app/protocol.ts`).
    pub fn say(&mut self, line: &str) {
        unsafe { tokyo_interface::channel() }.receive(line);
    }

    /// What has arrived goes to the GPU, and the slots that hold it are ready.
    fn arrivals(&mut self, now: f64) -> bool {
        let mut any = false;
        for _ in 0..ARRIVALS {
            let Some((slot, cell, bytes)) = self.arrived.borrow_mut().pop_front() else { break };
            self.asked[slot] = u32::MAX;
            let s = &slots()[slot];
            // (a slot keeps its cell while it is wanted)
            if s.cell != cell || s.state.load(Ordering::Acquire) != WANTED {
                continue;
            }
            match bytes.and_then(|bytes| self.renderer.cell(&self.gpu, slot, &self.tables.near[cell as usize], &bytes)) {
                Ok(()) => {
                    s.state.store(READY, Ordering::Release);
                    self.cells_read += 1;
                    any = true;
                }
                Err(e) => {
                    // It is asked for again in a second.
                    self.trouble = e;
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

    /// The shadows follow the sun: a sweep when it has moved a degree, or the shade has changed.
    fn shadows(&mut self, view: &View, busy: bool) {
        let (sun, shade) = (view.sun, view.shade);
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
            self.renderer.shadows(&self.gpu, texels);
            self.sweeps += 1;
            self.sweep_ms = ms;
        }
    }

    /// One frame at `now` (milliseconds on a clock that goes forward): the flight, what has arrived, the draws.
    pub fn frame(&mut self, now: f64, pad: &Pad) -> Result<(), String> {
        let from = task::now();
        let interval = self.last.map(|last| (now - last) as f32);
        self.last = Some(now);
        let ticks = self.ticks(interval.unwrap_or(1000.0 / 60.0));
        let mut view = View::default();
        unsafe {
            // (a frame that no tick has passed for shows the last one's flight again)
            if ticks > 0 {
                tokyo_core::tk_step(pad, ticks);
            }
            tokyo_core::tk_report(&self.perf);
            tokyo_core::tk_view(&mut view);
        }
        let busy = self.arrivals(now);
        self.shadows(&view, busy);
        let mut lists: [*const Item; KINDS] = [core::ptr::null(); KINDS];
        let mut lengths = [0u32; KINDS];
        unsafe {
            tokyo_core::tk_choose(lists.as_mut_ptr(), lengths.as_mut_ptr());
            tokyo_core::tk_refill();
        }
        self.ask(now);
        // (the lists are the core's until it chooses again)
        let lists: [&[Item]; KINDS] = core::array::from_fn(|k| if lengths[k] == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(lists[k], lengths[k] as usize) } });
        self.stats = self.renderer.frame(&self.gpu, &self.screen, self.tables, &view, lists)?;
        tokyo_core::tk_drew(self.stats.tris.iter().sum());

        let p = &mut self.perf;
        if let Some(interval) = interval {
            self.intervals[p.frames as usize % WINDOW] = interval;
            self.average += (interval - self.average) * 0.05;
            // A frame is shown for its refreshes; one that took half a refresh more is late.
            p.late += (interval > (self.shape.pace() as f32 + 0.5) * 1000.0 / 60.0) as u32;
        }
        p.frames += 1;
        p.frame = self.average;
        p.worst = self.intervals.iter().take(p.frames as usize).fold(0.0, |a, &b| a.max(b));
        p.cpu = (task::now() - from) as f32;
        p.draws = self.stats.draws;
        p.tris = self.stats.tris;
        Ok(())
    }

    /// Ticks of the flight (sixtieths of a second) for a frame that comes `interval` milliseconds after the
    /// last. The handhelds' displays refresh 60 times a second and a frame there is a whole number of ticks;
    /// so it is here when the time between two frames is within a twentieth of a whole number of them (a
    /// display of 59.94 a second, or of 120 with every second refresh drawn), and a late frame catches up.
    /// On any other display the part of a tick left over is owed to the next frame, so the flight keeps
    /// its speed.
    fn ticks(&mut self, interval: f32) -> u32 {
        let most = 3.max(self.shape.pace()) as f32;
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

    /// Whether every cell the eye is near is in its slot: nothing is wanted and nothing is on its way.
    pub fn settled(&self) -> bool {
        self.reading.get() == 0 && self.arrived.borrow().is_empty() && slots().iter().all(|s| s.state.load(Ordering::Acquire) != WANTED)
    }

    /// The run as a JSON object: the core's status (`tk_status`) with this shell's members.
    pub fn status(&self) -> String {
        let read = self.source.read_so_far();
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
            "\"fps\":{:.3},\"shape\":{{\"name\":\"{}\",\"width\":{},\"height\":{},\"samples\":{},\"budget\":{},\"hz\":{}}},\"adapter\":\"{}\",\"slotBytes\":{},\"read\":{{\"requests\":{},\"bytes\":{},\"piece\":{},\"cells\":{},\"underWay\":{}}},\"shadows\":{{\"sweeps\":{},\"ms\":{:.2}}},\"trouble\":\"{}\"",
            if self.perf.frame > 0.0 { 1000.0 / self.perf.frame } else { 0.0 },
            s.name,
            s.width,
            s.height,
            s.samples,
            s.budget,
            s.hz,
            self.gpu.adapter.replace(['"', '\\'], " "),
            self.tables.slot_bytes,
            read.requests,
            read.bytes,
            // (0: the pack is one file, read a range at a time)
            self.source.piece().unwrap_or(0),
            self.cells_read,
            self.reading.get(),
            self.sweeps,
            self.sweep_ms,
            trouble
        );
        let mut out = vec![0u8; 4096];
        let n = unsafe { tokyo_core::tk_status(out.as_mut_ptr(), out.len() as u32, &self.perf, extra.as_ptr(), extra.len() as u32) };
        out.truncate(n as usize);
        String::from_utf8(out).unwrap_or_default()
    }

    /// The place's name and its sources, as the pack's `META` gives them (JSON).
    pub fn meta(&self) -> &str {
        core::str::from_utf8(&self.tables.meta).unwrap_or("{}")
    }
}
