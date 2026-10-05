//! The tab's side: what the page calls (`page/main.js`).
//!
//! The page plays the Pocket3D title card, gives this a canvas and the pack's
//! URL, and then calls `frame` from its frame loop with the pad as its keys
//! make it. Everything a frame does is in `app`.
//!
//! The shadows are swept in a worker of the page (`page/sweep.js`), which
//! runs this module again with the city's heights alone ([`Sweeps`]): a sweep
//! takes a quarter of a frame's time on a fast machine and more than a frame
//! on a slow one, and the frames do not wait for it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::source::Source;
use tokyo_core::Pad;
use wasm_bindgen::prelude::*;

use crate::app::{App, Shape, SweepHere, Sweeper, SHAPES};
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

#[wasm_bindgen]
impl Tokyo {
    /// Reads the head of the pack at `url` and starts the flight on `canvas`, which has the shape's size in
    /// pixels. One to a page.
    ///
    /// `worker` runs `page/sweep.js` and sweeps the shadows; without one, or when it fails, the frames do.
    pub async fn start(canvas: web_sys::HtmlCanvasElement, url: String, shape: String, worker: Option<web_sys::Worker>) -> Result<Tokyo, JsError> {
        std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&info.to_string().into())));
        let shape = Shape::named(&shape).ok_or_else(|| JsError::new("no such shape"))?;
        let (gpu, surface) = Gpu::for_canvas(canvas).await.map_err(|e| JsError::new(&e))?;
        let screen = Screen::canvas(&gpu, surface, shape.width, shape.height, shape.samples);
        let sweeper = |tables: &'static Tables| -> Box<dyn Sweeper> {
            match worker {
                Some(worker) => Box::new(SweepBeside::new(worker, tables)),
                None => Box::new(SweepHere::new(tables)),
            }
        };
        let app = App::start(gpu, screen, shape, Source::new(&url), sweeper).await.map_err(|e| JsError::new(&e))?;
        Ok(Tokyo { app })
    }

    /// One frame at `now` (the frame loop's clock, milliseconds). `buttons`: `tokyo_sim::camera::btn` bits;
    /// `keys`: `tokyo_sim::flight::key` bits and `tokyo_interface::pad::MENU`; the sticks in -1…1, ahead and
    /// right positive.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(&mut self, now: f64, buttons: u32, keys: u32, lx: f32, ly: f32, rx: f32, ry: f32) -> Result<(), JsError> {
        self.app.frame(now, &Pad { buttons, keys, lx, ly, rx, ry }).map_err(|e| {
            self.app.trouble = e.clone();
            JsError::new(&e)
        })
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
    /// view=x,y,z,tx,ty,tz view=off near= mid= budget= option=`.
    pub fn control(&mut self, words: &str) {
        self.app.control(words);
    }

    /// A finger dragging the picture: pixels of a screen 480 wide since the last call.
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.app.say(&format!("{{\"type\":\"look\",\"dx\":{dx},\"dy\":{dy}}}"));
    }

    /// `frames` frames one after the other without waiting for the display, each a sixtieth of a second of
    /// the flight after the one before, from `now`: for a measurement, and for a picture of the canvas.
    pub fn burst(&mut self, now: f64, frames: u32) -> Result<(), JsError> {
        let pad = Pad { buttons: 0, keys: 0, lx: 0.0, ly: 0.0, rx: 0.0, ry: 0.0 };
        for i in 0..frames {
            self.app.frame(now + (i + 1) as f64 * 1000.0 / 60.0, &pad).map_err(|e| JsError::new(&e))?;
        }
        Ok(())
    }

    /// The run as a JSON object.
    pub fn status(&self) -> String {
        self.app.status()
    }

    /// Whether every cell the eye is near has arrived.
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
