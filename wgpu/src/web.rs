//! The tab's side: what the page calls (`page/main.js`).
//!
//! The page plays the Pocket3D title card, gives this a canvas and the pack's
//! URL, and then calls `frame` from its frame loop with the pad as its keys
//! make it. Everything a frame does is in `app`.

use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::source::Source;
use tokyo_core::Pad;
use wasm_bindgen::prelude::*;

use crate::app::{App, Shape, SweepHere, SHAPES};

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
    pub async fn start(canvas: web_sys::HtmlCanvasElement, url: String, shape: String) -> Result<Tokyo, JsError> {
        std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&info.to_string().into())));
        let shape = Shape::named(&shape).ok_or_else(|| JsError::new("no such shape"))?;
        let (gpu, surface) = Gpu::for_canvas(canvas).await.map_err(|e| JsError::new(&e))?;
        let screen = Screen::canvas(&gpu, surface, shape.width, shape.height, shape.samples);
        let app = App::start(gpu, screen, shape, Source::new(&url), |tables| Box::new(SweepHere::new(tables))).await.map_err(|e| JsError::new(&e))?;
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
