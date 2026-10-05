//! The tab's side: what the page calls (`page/main.js`).
//!
//! The page plays the Pocket3D title card and opens the shell on a canvas
//! ([`Tokyo::open`]); the city is read beside the frames ([`Reader::read`])
//! and handed in when it is there ([`Tokyo::fly`]). A frame is `step`, the
//! turn of the interface's guest, which the page runs in a realm of its own
//! and whose lines pass through `heard` and `say`, then `draw`. When what the
//! guest shows has changed, the page hands its picture to `overlay`.
//! Everything a frame does is in `app`.
//!
//! The shadows are swept in a worker of the page (`page/sweep.js`), which
//! runs this module again with the city's heights alone ([`Sweeps`]): a sweep
//! takes a quarter of a frame's time on a fast machine and more than a frame
//! on a slow one, and the frames do not wait for it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::source::Source;
use pocket_web_wgpu::wgpu::TextureFormat;
use wasm_bindgen::prelude::*;

use crate::app::{self, App, Held, Shape, SweepHere, Sweeper, SHAPES};
use crate::pack::Tables;

fn describe(s: &Shape) -> String {
    format!("{{\"name\":\"{}\",\"width\":{},\"height\":{},\"samples\":{},\"budget\":{},\"hz\":{}}}", s.name, s.width, s.height, s.samples, s.budget, s.hz)
}

/// The screens the page can ask for, as a JSON array.
#[wasm_bindgen]
pub fn shapes() -> String {
    format!("[{}]", SHAPES.iter().map(describe).collect::<Vec<_>>().join(","))
}

#[wasm_bindgen]
pub struct Tokyo {
    app: App,
}

/// What reads the city beside the frames: the GPU the shell draws with, and the screen its programs are for.
#[wasm_bindgen]
pub struct Reader {
    gpu: Gpu,
    format: TextureFormat,
    samples: u32,
}

/// The city once the pack's head has been read.
#[wasm_bindgen]
pub struct City {
    city: app::City,
}

#[wasm_bindgen]
impl Reader {
    /// Reads the head of the pack at `url`: the pack's file, on a server that answers byte ranges, or the
    /// manifest (`.json`) of a pack cut into pieces (`pocket_web_wgpu::source`).
    pub async fn read(self, url: String) -> Result<City, JsError> {
        let source = Source::open(&url).await.map_err(|e| JsError::new(&e))?;
        let city = app::City::read(self.gpu, self.format, self.samples, source).await.map_err(|e| JsError::new(&e))?;
        Ok(City { city })
    }
}

#[wasm_bindgen]
impl Tokyo {
    /// The shell on `canvas`, which has the shape's size in pixels, with no city yet: its frames show the
    /// interface alone. `prefs`: the settings the page kept from the last visit. One to a page.
    pub async fn open(canvas: web_sys::HtmlCanvasElement, shape: String, prefs: String) -> Result<Tokyo, JsError> {
        std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&info.to_string().into())));
        let shape = Shape::named(&shape).ok_or_else(|| JsError::new("no such shape"))?;
        let (gpu, surface) = Gpu::for_canvas(canvas).await.map_err(|e| JsError::new(&e))?;
        let screen = Screen::canvas(&gpu, surface, shape.width, shape.height, shape.samples);
        let mut app = App::open(gpu, screen, shape);
        if !prefs.is_empty() {
            app.prefs_stored(&prefs);
        }
        Ok(Tokyo { app })
    }

    /// What reads the city for this shell.
    pub fn reader(&self) -> Reader {
        Reader { gpu: self.app.gpu.clone(), format: self.app.screen.format, samples: self.app.screen.samples }
    }

    /// The city has been read: the flight starts. `worker` runs `page/sweep.js` and sweeps the shadows;
    /// without one, or when it fails, the frames do.
    pub fn fly(&mut self, city: City, worker: Option<web_sys::Worker>) -> Result<(), JsError> {
        let sweeper = |tables: &'static Tables| -> Box<dyn Sweeper> {
            match worker {
                Some(worker) => Box::new(SweepBeside::new(worker, tables)),
                None => Box::new(SweepHere::new(tables)),
            }
        };
        self.app.fly(city.city, sweeper).map_err(|e| JsError::new(&e))
    }

    /// Whether the city flies: its head has been read and handed in.
    pub fn flies(&self) -> bool {
        self.app.flies()
    }

    /// The start failed: the interface says why.
    pub fn fail(&mut self, why: &str) {
        self.app.fail(why);
    }

    /// The first half of a frame at `now` (the frame loop's clock, milliseconds): what the interface asked
    /// for, then the flight. `buttons`: PocketJS's bits of the buttons held on the page's handheld; the
    /// sticks in -1…1, right and up positive. Returns the sixtieths of a second that passed.
    pub fn step(&mut self, now: f64, buttons: u32, lx: f32, ly: f32, rx: f32, ry: f32) -> u32 {
        self.app.step(now, &Held { buttons, left: [lx, ly], right: [rx, ry] })
    }

    /// The second half: the scene, with the interface's picture over it.
    pub fn draw(&mut self) -> Result<(), JsError> {
        self.app.draw().map_err(|e| {
            self.app.trouble = e.clone();
            JsError::new(&e)
        })
    }

    /// Reads beside the frames while none is drawn: the title card is playing.
    pub fn pump(&mut self) {
        self.app.pump();
    }

    /// A guest has opened the interface's channel; one that replaces another opens it again.
    pub fn interface_opened(&mut self) {
        self.app.interface_opened();
    }

    /// Whether the guest's next turn is worth taking. `buttons`: PocketJS's bits, as the guest would be
    /// handed them; `touching`: a contact is on a surface it draws, or has just left one.
    pub fn guest_due(&self, buttons: u32, touching: bool) -> bool {
        self.app.guest_due(buttons, touching)
    }

    /// The line of state the guest has not seen, for its turn.
    pub fn heard(&mut self) -> Option<String> {
        self.app.heard()
    }

    /// A line the guest sent.
    pub fn say(&mut self, line: &str) {
        self.app.say(line);
    }

    /// The interface's picture as PocketJS's UI core rasterizes it with its alpha (`width` by `height` rows
    /// of premultiplied RGBA): it is laid over every frame from the next on.
    pub fn overlay(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<(), JsError> {
        self.app.overlay.write(&self.app.gpu, pixels, width, height).map_err(|e| JsError::new(&e))
    }

    /// Nothing is laid over the frames until a picture is handed in again: the guest is being replaced.
    pub fn overlay_hide(&mut self) {
        self.app.overlay.hide();
    }

    /// What the interface asked to have kept since the last call (its settings, as JSON text).
    pub fn prefs_take(&mut self) -> Option<String> {
        self.app.prefs_take()
    }

    /// Another screen from the next frame on. The canvas has the new size already. `name` is one of
    /// `shapes()`; a number that is not zero replaces that shape's.
    pub fn reshape(&mut self, name: &str, width: u32, height: u32, samples: u32, budget: u32, hz: u32) -> Result<String, JsError> {
        let mut shape = Shape::named(name).ok_or_else(|| JsError::new("no such shape"))?;
        let or = |value: u32, fallback: u32| if value != 0 { value } else { fallback };
        shape = Shape { name: shape.name, width: or(width, shape.width), height: or(height, shape.height), samples: or(samples, shape.samples), budget: or(budget, shape.budget), hz: or(hz, shape.hz) };
        self.app.reshape(shape);
        Ok(describe(&self.app.shape))
    }

    /// Words for the flow and for the flight, as a development host sends them: `tour=1 hour=18.5 rate=0
    /// view=x,y,z,tx,ty,tz view=off near= mid= budget= option= mode=flight ui=menu`.
    pub fn control(&mut self, words: &str) {
        self.app.control(words);
    }

    /// The run as a JSON object.
    pub fn status(&self) -> String {
        self.app.status()
    }

    /// Whether the city flies, and everything the eye is near has arrived.
    pub fn settled(&self) -> bool {
        self.app.settled()
    }

    /// The pack's `META`: the place's name and its sources (JSON).
    pub fn meta(&self) -> String {
        self.app.meta().into()
    }
}

// ---------------------------------------------------------------- the shadows, beside the frames

/// What a worker hands back: a sweep's texels and the milliseconds it took.
type Swept = Rc<RefCell<Option<(Vec<u8>, f32)>>>;

/// Sweeps in a worker of the page. The first sweep is the first frame's own, so that no frame shows the city
/// without its shadows; so is every sweep when the worker fails.
struct SweepBeside {
    worker: web_sys::Worker,
    /// 0: the worker is starting; 1: it sweeps; 2: it failed.
    state: Rc<Cell<u8>>,
    busy: Rc<Cell<bool>>,
    swept: Swept,
    held: Option<(Vec<u8>, f32)>,
    here: SweepHere,
    /// A sweep of the frames' own has not been read yet; none has been made.
    mine: bool,
    first: bool,
    _heard: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _failed: Closure<dyn FnMut(web_sys::Event)>,
}

impl SweepBeside {
    fn new(worker: web_sys::Worker, tables: &'static Tables) -> SweepBeside {
        let (state, busy, swept): (Rc<Cell<u8>>, Rc<Cell<bool>>, Swept) = Default::default();
        let heard = {
            let (state, busy, swept) = (state.clone(), busy.clone(), swept.clone());
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
                let data = event.data();
                let member = |name: &str| js_sys::Reflect::get(&data, &name.into()).unwrap_or(JsValue::UNDEFINED);
                if member("ready").is_truthy() {
                    state.set(1);
                } else if let Some(texels) = member("texels").dyn_ref::<js_sys::Uint8Array>() {
                    *swept.borrow_mut() = Some((texels.to_vec(), member("ms").as_f64().unwrap_or(0.0) as f32));
                    busy.set(false);
                }
            })
        };
        let failed = {
            let state = state.clone();
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| state.set(2))
        };
        worker.set_onmessage(Some(heard.as_ref().unchecked_ref()));
        worker.set_onerror(Some(failed.as_ref().unchecked_ref()));
        // The worker needs the city's frame and its heights, and nothing else of the pack.
        let start = js_sys::Object::new();
        let city = js_sys::Uint8Array::from(tokyo_pack::bytes_of(&tables.city));
        let heights = js_sys::Uint8Array::from(tokyo_pack::slice_bytes(&tables.heights));
        let sent = js_sys::Reflect::set(&start, &"city".into(), &city).is_ok() && js_sys::Reflect::set(&start, &"heights".into(), &heights).is_ok() && worker.post_message(&start).is_ok();
        if !sent {
            state.set(2);
        }
        SweepBeside { worker, state, busy, swept, held: None, here: SweepHere::new(tables), mine: false, first: true, _heard: heard, _failed: failed }
    }
}

impl Sweeper for SweepBeside {
    fn start(&mut self, sun: [f32; 3], shade: f32) -> bool {
        if self.first || self.state.get() == 2 {
            self.first = false;
            self.mine = true;
            return self.here.start(sun, shade);
        }
        if self.state.get() == 0 || self.busy.get() {
            return false;
        }
        let words = js_sys::Float32Array::from(&[sun[0], sun[1], sun[2], shade][..]);
        if self.worker.post_message(&words).is_err() {
            self.state.set(2);
            return false;
        }
        self.busy.set(true);
        true
    }

    fn finished(&mut self) -> Option<(&[u8], f32)> {
        if self.mine {
            self.mine = false;
            return self.here.finished();
        }
        self.held = self.swept.borrow_mut().take();
        self.held.as_ref().map(|(texels, ms)| (&texels[..], *ms))
    }
}

/// The worker's side: the core with the city's frame and its heights, and no flight to speak of.
#[wasm_bindgen]
pub struct Sweeps {
    here: SweepHere,
}

#[wasm_bindgen]
impl Sweeps {
    /// `city`: the pack's `CITY` record; `heights`: its `HMAP`, as bytes.
    #[wasm_bindgen(constructor)]
    pub fn new(city: &[u8], heights: &[u8]) -> Result<Sweeps, JsError> {
        let city: tokyo_pack::City = tokyo_pack::read(city, 0).ok_or_else(|| JsError::new("the city's record is cut short"))?;
        let heights: Vec<u16> = tokyo_pack::table(heights);
        if heights.len() < (city.grid_w * city.grid_h) as usize {
            return Err(JsError::new("the heights are cut short"));
        }
        let tables: &'static Tables = Box::leak(Box::new(Tables {
            city,
            regions: Vec::new(),
            blocks: Vec::new(),
            cells: Vec::new(),
            batches: Vec::new(),
            spans: Vec::new(),
            tour: Vec::new(),
            heights,
            near: Vec::new(),
            landmarks: Vec::new(),
            meta: Vec::new(),
            near_offset: 0,
            slot_bytes: 0,
            pictures: Vec::new(),
            texels_offset: 0,
        }));
        let refusal = unsafe { tokyo_core::tk_init(&tables.for_core(), 0) };
        if !refusal.is_null() {
            return Err(JsError::new("the city is not a handheld's"));
        }
        Ok(Sweeps { here: SweepHere::new(tables) })
    }

    /// The shadows for a direction of the sun: 1 024 squared texels of 8 bits, in rows.
    pub fn sweep(&mut self, x: f32, y: f32, z: f32, shade: f32) -> Vec<u8> {
        self.here.sweep([x, y, z], shade).to_vec()
    }
}
