//! The interface: one PocketJS guest (`ui/`, shared with the other devices)
//! drawn over the city through PocketJS's PSP host library. It owns every 2D
//! pixel, and what the buttons mean while a list is up; this side owns the
//! scene, shows it the flight's state and does what it asks
//! (`crates/tokyo-interface`).
//!
//! The guest is offered a turn every frame, 30 times a second, while the GE
//! draws the previous frame: the pad goes in, the guest's script runs, and
//! the UI core lays out what it shows. The frame's own work on this CPU is a
//! third of what the GE takes for the same frame, so a whole turn fits in the
//! wait. A frame without a turn draws the same list again.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

use libquickjs_sys::*;
use tokyo_interface::{channel, guest, Mode, Pace, Session};
use pocketjs_psp::{arena, ffi, ge, host, pak, qjs_alloc};
use psp::sys::sceKernelGetSystemTimeLow;

// libquickjs-sys omits these; the linked QuickJS provides them.
extern "C" {
    fn JS_NewArrayBuffer(ctx: *mut JSContext, buf: *mut u8, len: usize, free: Option<unsafe extern "C" fn(*mut JSRuntime, *mut c_void, *mut c_void)>, opaque: *mut c_void, shared: i32) -> JSValue;
    fn JS_RunGC(rt: *mut JSRuntime);
}

/// The guest's turns a second; it is told so before it mounts. The UI core counts sixtieths, so a
/// turn is this many of its ticks.
const TURNS: u32 = 30;
const TICKS: u32 = 60 / TURNS;
/// Seconds between two turns.
pub const TURN: f32 = 1.0 / TURNS as f32;
/// The stick at rest, as the guest reads it: x in the high byte, y in the low.
pub const STICK_CENTER: u32 = 0x8080;
/// What the interface takes: its pak, the guest's script and heap, the UI core and its working
/// memory. On the console it stands at 5.0 MB once the title is up and at 5.7 MB once every list
/// has been opened. Memory must have this much left over after the city for the interface to start.
pub const RESERVE: usize = 6 * 1024 * 1024;

pub struct Ui {
    guest: Option<(*mut JSRuntime, *mut JSContext, JSValue, JSValue)>,
    /// Seconds the guest is owed.
    owed: f32,
    /// Buttons held at any frame since the last turn.
    latched: u32,
    /// The arena's high-water mark at the last collection.
    collected: usize,
    /// What it last drew, for the frames between its turns.
    words: (*const u32, usize),
    /// Which of the turns it is offered are taken.
    pace: Pace,
    /// Buttons a control message presses, and for how many more turns: a press is held two turns and
    /// let go for one; a rest holds nothing.
    presses: VecDeque<(u32, u8)>,
    /// Why there is no interface, or what its script last threw.
    pub error: String,
    /// Milliseconds of a turn, smoothed over the last ones: the guest's script, then the core's
    /// layout and its list.
    pub script_ms: f32,
    pub layout_ms: f32,
    /// The longest turn so far, milliseconds, and the longest since `recent` last asked.
    pub worst_ms: f32,
    /// The longest turn since `recent` last asked: its script, its layout, and of its script the collection.
    recent_ms: [f32; 3],
    /// Collections of the guest's heap so far, and milliseconds the last one took.
    pub collections: u32,
    pub collect_ms: f32,
    pub turns: u32,
    /// Bytes of the arena the guest took to start.
    pub bytes: usize,
}

/// The pending exception as text.
unsafe fn exception(ctx: *mut JSContext) -> String {
    let e = JS_GetException(ctx);
    let mut out = String::new();
    let mut len: size_t = 0;
    let s = JS_ToCStringLen2(ctx, &mut len, e, 0);
    if !s.is_null() {
        if let Ok(text) = core::str::from_utf8(core::slice::from_raw_parts(s as *const u8, len)) {
            out.extend(text.chars().take(160).map(|c| if c == '"' || c == '\\' || c < ' ' { ' ' } else { c }));
        }
        JS_FreeCString(ctx, s);
    }
    JS_FreeValue(ctx, e);
    out
}

impl Ui {
    /// No interface: the pad keeps the flow itself.
    pub fn none(why: &str) -> Ui {
        unsafe { channel().close() };
        Ui { guest: None, owed: TURN, latched: 0, collected: 0, words: (core::ptr::null(), 0), pace: Pace::default(), presses: VecDeque::new(), error: why.into(), script_ms: 0.0, layout_ms: 0.0, worst_ms: 0.0, recent_ms: [0.0; 3], collections: 0, collect_ms: 0.0, turns: 0, bytes: 0 }
    }

    /// Boots the guest: `script` is the bundle, NUL-terminated (not needed once this returns), `pak`
    /// its styles, fonts and pictures, which the guest borrows for good.
    pub unsafe fn boot(script: Option<Vec<u8>>, pak: Option<&'static [u8]>) -> Ui {
        let (Some(script), Some(pak)) = (script, pak) else {
            return Ui::none("tokyo.js or tokyo.pak is missing");
        };
        let core = ffi::init_ui();
        let (textures, sprites) = pak::feed(core, pak);
        pak::install(pak);
        let rt = qjs_alloc::new_runtime();
        let ctx = if rt.is_null() { core::ptr::null_mut() } else { JS_NewContext(rt) };
        if ctx.is_null() {
            return Ui::none("no memory for the interface");
        }
        let global = JS_GetGlobalObject(ctx);
        ffi::register(ctx, global, &textures, &sprites);
        // The channel to this renderer takes the place of the host's wire.
        guest::mount(ctx, global);
        JS_SetPropertyStr(ctx, global, c"__simHz".as_ptr(), JS_NewInt32(ctx, TURNS as i32));
        JS_SetPropertyStr(ctx, global, c"__pak".as_ptr(), JS_NewArrayBuffer(ctx, pak.as_ptr() as *mut u8, pak.len(), None, core::ptr::null_mut(), 0));
        let result = JS_Eval(ctx, script.as_ptr() as *const _, script.len() - 1, c"tokyo.js".as_ptr(), JS_EVAL_TYPE_GLOBAL as i32);
        let thrown = (JS_ValueGetTag(result) == JS_TAG_EXCEPTION).then(|| exception(ctx));
        JS_FreeValue(ctx, result);
        let frame = JS_GetPropertyStr(ctx, global, c"frame".as_ptr());
        if thrown.is_some() || JS_IsUndefined(frame) {
            let mut ui = Ui::none("the interface did not start");
            if let Some(text) = thrown.filter(|t| !t.is_empty()) {
                ui.error = text;
            }
            return ui;
        }
        host::drain_jobs(rt);
        JS_RunGC(rt);
        Ui { guest: Some((rt, ctx, global, frame)), owed: TURN, latched: 0, collected: arena::stats().bump_bytes, words: (core::ptr::null(), 0), pace: Pace::default(), presses: VecDeque::new(), error: String::new(), script_ms: 0.0, layout_ms: 0.0, worst_ms: 0.0, recent_ms: [0.0; 3], collections: 0, collect_ms: 0.0, turns: 0, bytes: 0 }
    }

    /// The guest is on the screen.
    pub fn up(&self) -> bool {
        self.guest.is_some()
    }

    /// Presses `buttons` on the interface as a thumb would, after the presses already waiting.
    pub fn press(&mut self, buttons: u32) {
        self.presses.push_back((buttons, 3));
    }

    /// Leaves the pad alone for `turns` before the next waiting press.
    pub fn rest(&mut self, turns: u32) {
        self.presses.push_back((0, turns.clamp(1, 255) as u8));
    }

    /// The guest's turn when `dt` more seconds make one due: the pad goes in (a button held at any
    /// frame since the last turn counts), its script runs, and what it shows is laid out. What it
    /// asked for waits in the channel for `Session::obey`.
    ///
    /// `session` says whether the turn is worth taking: one costs this CPU several milliseconds
    /// however little changed, so an idle guest is turned only when there is news or a button it
    /// listens to moved.
    pub unsafe fn turn(&mut self, dt: f32, buttons: u32, analog: u32, session: &Session) {
        self.latched |= buttons;
        self.owed = (self.owed + dt).min(2.0 * TURN);
        let Some((rt, ctx, global, frame)) = self.guest else { return };
        if self.owed < TURN {
            return;
        }
        self.owed -= TURN;
        if !self.pace.due(session, self.latched, false) && self.presses.is_empty() {
            self.latched = 0;
            return;
        }
        let start = sceKernelGetSystemTimeLow();
        let mut buttons = core::mem::take(&mut self.latched);
        if let Some((pressed, turns)) = self.presses.front_mut() {
            *turns -= 1;
            // The last turn of a press is the one that lets go.
            if *turns > 0 || *pressed == 0 {
                buttons |= *pressed;
            }
            if *turns == 0 {
                self.presses.pop_front();
            }
        }
        let mut arguments = [JS_NewInt32(ctx, buttons as i32), JS_NewInt32(ctx, analog as i32)];
        let result = JS_Call(ctx, frame, global, 2, arguments.as_mut_ptr());
        if JS_ValueGetTag(result) == JS_TAG_EXCEPTION {
            self.error = exception(ctx);
        }
        JS_FreeValue(ctx, result);
        host::drain_jobs(rt);
        // Collect when the arena stands a quarter megabyte higher than at the last collection (as
        // PocketJS's own host does): a steady interface never does. A collection stops this CPU for
        // 60 to 80 ms, two frames of the city, so it waits for a list to be up, where the frame it
        // costs is one of a list changing; over a megabyte it does not wait.
        let bump = arena::stats().bump_bytes;
        let mut collect = 0.0;
        if bump > self.collected + if session.mode == Mode::Flight { 1024 * 1024 } else { 256 * 1024 } {
            let from = sceKernelGetSystemTimeLow();
            JS_RunGC(rt);
            self.collected = arena::stats().bump_bytes;
            collect = sceKernelGetSystemTimeLow().wrapping_sub(from) as f32 / 1000.0;
            self.collections += 1;
            self.collect_ms = collect;
        }
        let scripted = sceKernelGetSystemTimeLow();
        let core = ffi::ui();
        for _ in 0..TICKS {
            core.tick();
        }
        let list = core.draw();
        self.words = (list.words.as_ptr(), list.words.len());
        let (script, layout) = (scripted.wrapping_sub(start) as f32 / 1000.0, sceKernelGetSystemTimeLow().wrapping_sub(scripted) as f32 / 1000.0);
        self.turns += 1;
        // The first turns mount the screens; the figures are for the ones after.
        if self.turns > 8 {
            self.script_ms += (script - self.script_ms) * 0.1;
            self.layout_ms += (layout - self.layout_ms) * 0.1;
            self.worst_ms = self.worst_ms.max(script + layout);
            if script + layout > self.recent_ms[0] + self.recent_ms[1] {
                self.recent_ms = [script, layout, collect];
            }
        }
    }

    /// Draws the interface into the open display list, over what is there. The list's vertices come
    /// from the host library's pool: `ge::reset_pool` once the GE has drawn them.
    pub unsafe fn draw(&self) {
        if self.guest.is_some() && self.words.1 > 0 {
            ge::render_over(ffi::ui(), core::slice::from_raw_parts(self.words.0, self.words.1));
        }
    }

    /// The longest turn since this was last asked, milliseconds: its script, its layout, and of its
    /// script the collection of the guest's heap.
    pub fn recent(&mut self) -> [f32; 3] {
        core::mem::take(&mut self.recent_ms)
    }

    /// The length of the list it last drew, in 32-bit words.
    pub fn words(&self) -> usize {
        self.words.1
    }

    /// Bytes QuickJS holds for the guest.
    pub unsafe fn script_bytes(&self) -> usize {
        if self.guest.is_some() {
            qjs_alloc::stats().live_requested
        } else {
            0
        }
    }
}
