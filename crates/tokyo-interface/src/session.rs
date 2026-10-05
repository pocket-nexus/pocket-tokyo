//! The flow around a flight, the same on every device: the tour behind the
//! title, the flight with the tour or the pad carrying the eye, and the menu
//! over it. A device owns the pad's wiring and the scene; it hands this its
//! [`Pad`] each frame and does what the flight then says.

use alloc::string::String;
use alloc::vec::Vec;

use tokyo_sim::camera::Input;
use tokyo_sim::flight::Flight;
use tokyo_sim::math::*;

use crate::{channel, Command, Mode, Setting, State, Telemetry};

/// The pad as a device hands it over, whatever its buttons are called there.
pub mod pad {
    pub use tokyo_sim::camera::btn::{DOWN, FAST, UP};
    pub use tokyo_sim::flight::key::{EARLIER, LATER, TOUR};
    /// The button that opens the menu. The interface hears it by itself; this bit is for a device
    /// with no interface on the screen, where it hands the eye to the tour and takes it back.
    pub const MENU: u32 = 1 << 16;
    /// The bits the camera reads, and the ones the flight reads.
    pub const CAMERA: u32 = FAST | UP | DOWN;
    pub const KEYS: u32 = TOUR | LATER | EARLIER;
}

/// Sticks in -1…1 (forward and right positive) and [`pad`] bits.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Pad {
    pub buttons: u32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
}

/// How fast Tokyo's clock runs by itself: what the setting calls it, and hours a second.
pub const FLOWS: [(&str, f32); 3] = [("Stopped", 0.0), ("Slow", 0.03), ("Fast", 0.25)];

/// Radians a finger turns the view per logical pixel it drags: the picture follows the finger at
/// the camera's field of view on a screen 480 pixels wide.
const LOOK: f32 = 0.003;
/// Hours a second the clock turns toward an hour the interface asked for.
const SWEEP: f32 = 9.0;

pub struct Session {
    pub mode: Mode,
    /// The stick's pitch is turned over.
    pub invert: bool,
    /// The device draws traffic and lets the person turn it off.
    pub traffic: bool,
    /// The interface has nothing scheduled: see [`Pace`].
    pub idle: bool,
    /// Frames between two refreshes of the numbers in flight.
    pub numbers_every: u32,
    flow: usize,
    hour_to: Option<f32>,
    /// A touch panel's stick and keys, and what a finger dragged since it was last spent.
    drive: (f32, f32, u32),
    look: (f32, f32),
    prev_buttons: u32,
    frames: u32,
    last_eye: Option<V3>,
    speed: f32,
    /// The settings as last listed for the interface.
    listed: Option<(bool, usize, bool, bool)>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Session {
        Session {
            mode: Mode::Title,
            invert: false,
            traffic: false,
            idle: false,
            numbers_every: 2,
            flow: 1,
            hour_to: None,
            drive: (0.0, 0.0, 0),
            look: (0.0, 0.0),
            prev_buttons: u32::MAX,
            frames: 0,
            last_eye: None,
            speed: 0.0,
            listed: None,
        }
    }

    /// Nothing a touch panel held carries over a change of mode.
    fn rest(&mut self) {
        self.drive = (0.0, 0.0, 0);
        self.look = (0.0, 0.0);
    }

    /// Does what the interface asked. A command that is the device's own comes back.
    pub fn command(&mut self, flight: &mut Flight, command: Command) -> Option<Command> {
        let has_tour = !flight.tour.is_empty();
        match command {
            Command::Start { tour } => {
                if self.mode == Mode::Title {
                    self.mode = Mode::Flight;
                    flight.tour_on = tour && has_tour;
                    self.rest();
                }
            }
            Command::Menu(on) => match (self.mode, on) {
                (Mode::Flight, true) => {
                    self.mode = Mode::Menu;
                    self.rest();
                }
                (Mode::Menu, false) => self.mode = Mode::Flight,
                _ => {}
            },
            Command::Tour(on) => {
                if self.mode != Mode::Title {
                    flight.tour_on = on && has_tour;
                    flight.view = None;
                }
            }
            Command::Hour(minutes) => self.hour_to = Some(clamp(minutes, 0.0, 1439.0) / 60.0),
            Command::Title => {
                self.mode = Mode::Title;
                flight.tour_on = has_tour;
                flight.view = None;
                self.rest();
            }
            Command::Option { key, value } => match key.as_str() {
                "invert" => self.invert = value != 0,
                "flow" => {
                    self.flow = (value as usize).min(FLOWS.len() - 1);
                    flight.rate = FLOWS[self.flow].1;
                }
                "stats" => flight.stats = value != 0,
                "traffic" if self.traffic => flight.traffic = value != 0,
                _ => return Some(Command::Option { key, value }),
            },
            Command::Drive { mx, my, buttons } => self.drive = (clamp(mx, -1.0, 1.0), clamp(my, -1.0, 1.0), buttons & pad::CAMERA),
            Command::Look { dx, dy } => {
                self.look.0 += dx * LOOK;
                self.look.1 += dy * LOOK;
            }
            Command::Idle(on) => self.idle = on,
            other => return Some(other),
        }
        None
    }

    /// Does everything the interface asked since the last frame. `other(key, value)` is told an
    /// option this flow does not know; the return is the text the interface wants kept, when it
    /// said so.
    pub fn obey(&mut self, flight: &mut Flight, mut other: impl FnMut(&str, u32)) -> Option<String> {
        let mut kept = None;
        while let Some(command) = unsafe { channel() }.next() {
            match self.command(flight, command) {
                Some(Command::Prefs(text)) => kept = Some(text),
                Some(Command::Option { key, value }) => other(&key, value),
                _ => {}
            }
        }
        kept
    }

    /// The settings this flow keeps, in menu order.
    pub fn settings(&self, flight: &Flight, out: &mut Vec<Setting>) {
        out.push(Setting::choice("flow", self.flow, &[FLOWS[0].0, FLOWS[1].0, FLOWS[2].0]));
        out.push(Setting::switch("invert", self.invert));
        if self.traffic {
            out.push(Setting::switch("traffic", flight.traffic));
        }
        out.push(Setting::switch("stats", flight.stats));
    }

    /// What the camera and the flight read this frame: nothing while the title or a list has the pad.
    fn input(&mut self, pad: &Pad, dt: f32) -> (Input, u32, (f32, f32)) {
        if self.mode != Mode::Flight {
            return (Input::default(), 0, (0.0, 0.0));
        }
        let (mx, my, held) = self.drive;
        let touched = mx != 0.0 || my != 0.0;
        let ry = if self.invert { -pad.ry } else { pad.ry };
        let inp = Input { buttons: (pad.buttons | held) & pad::CAMERA, lx: if touched { mx } else { pad.lx }, ly: if touched { my } else { pad.ly }, rx: pad.rx, ry };
        // What a finger dragged is spent over a few frames: the view follows it without a step.
        let k = min(1.0, 20.0 * dt);
        let turn = (self.look.0 * k, self.look.1 * k);
        self.look.0 -= turn.0;
        self.look.1 -= turn.1;
        if abs(self.look.0) < 1e-4 {
            self.look.0 = 0.0;
        }
        if abs(self.look.1) < 1e-4 {
            self.look.1 = 0.0;
        }
        (inp, pad.buttons & pad::KEYS, turn)
    }

    /// One frame of `dt` seconds. `top(x, z)`: the height of whatever stands at a point.
    pub fn run(&mut self, flight: &mut Flight, pad: &Pad, dt: f32, top: impl Fn(f32, f32) -> f32) {
        let pressed = pad.buttons & !self.prev_buttons;
        self.prev_buttons = pad.buttons;
        self.frames = self.frames.wrapping_add(1);
        // No interface on the screen: the city has it, and the menu button hands the eye to the
        // tour and takes it back.
        if !unsafe { channel() }.is_open() && self.frames > 30 {
            self.mode = Mode::Flight;
            if pressed & pad::MENU != 0 && !flight.tour.is_empty() {
                flight.tour_on = !flight.tour_on;
                flight.view = None;
            }
        }
        if self.mode == Mode::Title && !flight.tour.is_empty() {
            flight.tour_on = true;
        }
        // The clock on its way to an hour the interface asked for, by the shorter way round.
        if let Some(to) = self.hour_to {
            let mut d = to - flight.hour;
            d -= floor(d / 24.0 + 0.5) * 24.0;
            let step = clamp(d, -SWEEP * dt, SWEEP * dt);
            flight.hour += step;
            if abs(d - step) < 1e-3 {
                self.hour_to = None;
            }
        }
        let (inp, keys, turn) = self.input(pad, dt);
        if turn.0 != 0.0 || turn.1 != 0.0 {
            // A finger on the picture takes the eye off the tour, as a stick does.
            flight.tour_on = false;
            flight.cam.turn_by(turn.0, turn.1);
        }
        flight.step(&inp, keys, dt, top);
        let eye = flight.eye().0;
        if let Some(last) = self.last_eye {
            self.speed = ease(self.speed, (eye - last).len() / max(dt, 1e-3), 6.0, dt);
        }
        self.last_eye = Some(eye);
    }

    /// Writes what the interface is shown. The numbers in flight are refreshed every
    /// `numbers_every` frames.
    pub fn publish(&mut self, flight: &Flight, state: &mut State) {
        state.mode = self.mode;
        state.tour = flight.tour_on && flight.view.is_none() && !flight.tour.is_empty();
        let listed = (self.invert, self.flow, flight.stats, self.traffic && flight.traffic);
        if self.listed != Some(listed) {
            self.listed = Some(listed);
            state.options.clear();
            self.settings(flight, &mut state.options);
        }
        if self.frames % self.numbers_every.max(1) != 0 {
            return;
        }
        let (eye, look, _) = flight.eye();
        let whole = |x: f32| floor(x + 0.5) as i32;
        state.t = Telemetry {
            minutes: (flight.hour * 60.0) as i32 % 1440,
            altitude: whole(eye.y),
            speed: whole(self.speed * 3.6),
            heading: whole(atan2(look.x, -look.z) * (180.0 / PI)).rem_euclid(360),
            x: whole(eye.x),
            z: whole(eye.z),
        };
    }
}

/// PocketJS's START: the one button the interface listens to during a flight (it opens the menu).
const GUEST_MENU: u32 = 0x0008;
/// Turns the guest still takes after the last reason for one: what a press or a line started
/// (a list sliding in, a mark moving) plays out.
const LINGER: u8 = 12;

/// Which of the turns a device offers the guest are worth their cost. A turn runs the whole
/// framework's frame, milliseconds on the slower machines however little changed.
#[derive(Default)]
pub struct Pace {
    buttons: u32,
    linger: u8,
}

impl Pace {
    pub const fn new() -> Pace {
        Pace { buttons: 0, linger: 0 }
    }

    /// `buttons`: the guest's pad (PocketJS's bits) as it would be handed over this turn;
    /// `touching`: a finger is on a surface the guest draws.
    pub fn due(&mut self, session: &Session, buttons: u32, touching: bool) -> bool {
        let heard = if session.mode == Mode::Flight { buttons & GUEST_MENU } else { buttons };
        let changed = heard != self.buttons;
        self.buttons = heard;
        let interface = unsafe { channel() };
        let news = interface.news();
        if !session.idle || changed || touching || (news && !interface.only_readouts()) {
            self.linger = LINGER;
            return true;
        }
        if self.linger > 0 {
            self.linger -= 1;
            return true;
        }
        news
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn flight() -> Flight {
        Flight::new([0.0, 100.0, 0.0, 0.0, 0.0, -100.0], 12.0, vec![[0.0, 100.0, 0.0, 50.0, 0.0, 0.0], [500.0, 100.0, 0.0, 50.0, 0.0, 0.0]], 28_000, (250.0, 1200.0), 2)
    }

    fn flat(_: f32, _: f32) -> f32 {
        0.0
    }

    // One test: the channel is one for the process.
    #[test]
    fn the_flow_follows_the_interface() {
        let mut f = flight();
        let mut session = Session::new();
        unsafe { channel() }.open("pocket.overlay");
        let dt = 1.0 / 30.0;
        // Behind the title the tour flies whatever the pad does.
        session.run(&mut f, &Pad { buttons: pad::UP | pad::MENU, ly: 1.0, ..Pad::default() }, dt, flat);
        assert_eq!((session.mode, f.tour_on), (Mode::Title, true));
        assert_eq!(session.command(&mut f, Command::Start { tour: false }), None);
        assert_eq!((session.mode, f.tour_on), (Mode::Flight, false));
        // In flight the pad carries the eye.
        let before = f.cam.pos;
        for _ in 0..30 {
            session.run(&mut f, &Pad { ly: 1.0, ..Pad::default() }, dt, flat);
        }
        assert!((f.cam.pos - before).len() > 10.0);
        // Under a list it does not, and the tour's key is not heard.
        session.command(&mut f, Command::Menu(true));
        assert_eq!(session.mode, Mode::Menu);
        for _ in 0..60 {
            session.run(&mut f, &Pad { ly: 1.0, buttons: pad::TOUR, ..Pad::default() }, dt, flat);
        }
        let hover = f.cam.pos;
        session.run(&mut f, &Pad { ly: 1.0, ..Pad::default() }, dt, flat);
        assert!((f.cam.pos - hover).len() < 0.5 && !f.tour_on);
        // The clock turns to the hour asked for, by the shorter way.
        f.hour = 23.0;
        session.command(&mut f, Command::Hour(60.0));
        for _ in 0..30 {
            session.run(&mut f, &Pad::default(), dt, flat);
        }
        assert!((f.hour - 1.0).abs() < 0.05, "the clock reads {}", f.hour);
        // Settings: the flow of time is this flow's; one it does not know is the device's.
        assert_eq!(session.command(&mut f, Command::Option { key: "flow".into(), value: 0 }), None);
        assert_eq!(f.rate, 0.0);
        assert_eq!(session.command(&mut f, Command::Option { key: "stats".into(), value: 1 }), None);
        assert!(f.stats);
        let glow = Command::Option { key: "glow".into(), value: 0 };
        assert_eq!(session.command(&mut f, glow.clone()), Some(glow));
        let mut state = State::default();
        session.publish(&f, &mut state);
        assert_eq!(state.options, vec![Setting::choice("flow", 0, &["Stopped", "Slow", "Fast"]), Setting::switch("invert", false), Setting::switch("stats", true)]);
        assert_eq!(state.mode, Mode::Menu);
        // The tour takes the eye back, and the title is one command away.
        session.command(&mut f, Command::Tour(true));
        assert!(f.tour_on);
        session.command(&mut f, Command::Menu(false));
        session.command(&mut f, Command::Title);
        assert_eq!((session.mode, f.tour_on), (Mode::Title, true));

        // A touch panel: a finger drags the view, the stick and the keys fly.
        session.command(&mut f, Command::Start { tour: true });
        let yaw = f.cam.yaw;
        session.command(&mut f, Command::Look { dx: 100.0, dy: 0.0 });
        for _ in 0..30 {
            session.run(&mut f, &Pad::default(), dt, flat);
        }
        assert!(!f.tour_on, "a finger on the picture takes the eye off the tour");
        let turned = wrap_angle(f.cam.yaw - yaw);
        assert!((turned - 100.0 * LOOK).abs() < 0.02, "turned {turned}");
        session.command(&mut f, Command::Drive { mx: 0.0, my: 1.0, buttons: pad::UP | 0x100 });
        let (inp, _, _) = session.input(&Pad::default(), dt);
        assert_eq!(inp.buttons, pad::UP);
        assert!(inp.ly > 0.99);
        session.command(&mut f, Command::Menu(true));
        assert_eq!(session.input(&Pad::default(), dt).0.buttons, 0);
        session.command(&mut f, Command::Menu(false));

        // The numbers in flight.
        f.control("fly=120,300,-40,120,300,-1000");
        f.hour = 15.5;
        session.numbers_every = 1;
        session.run(&mut f, &Pad::default(), dt, flat);
        session.publish(&f, &mut state);
        assert_eq!((state.t.minutes, state.t.altitude, state.t.heading, state.t.x, state.t.z), (930, 300, 0, 120, -40));

        // An idle guest is turned when there is a reason.
        let mut pace = Pace::default();
        assert!(pace.due(&session, 0, false));
        session.command(&mut f, Command::Idle(true));
        *unsafe { &mut channel().state } = state.clone();
        unsafe { channel() }.poll();
        for _ in 0..LINGER {
            assert!(pace.due(&session, 0, false));
        }
        assert!(!pace.due(&session, 0, false));
        // In flight the interface hears the menu button alone.
        assert!(!pace.due(&session, 0x4000, false));
        assert!(pace.due(&session, GUEST_MENU, false));
        for _ in 0..LINGER {
            pace.due(&session, GUEST_MENU, false);
        }
        assert!(!pace.due(&session, GUEST_MENU, false));
        // New numbers are worth one turn; a finger on the panel is worth every turn.
        unsafe { channel() }.state.t.speed += 1;
        assert!(pace.due(&session, GUEST_MENU, false));
        unsafe { channel() }.poll();
        for _ in 0..LINGER {
            pace.due(&session, GUEST_MENU, false);
        }
        assert!(!pace.due(&session, GUEST_MENU, false));
        assert!(pace.due(&session, GUEST_MENU, true));

        // With no interface on the screen the pad keeps the flow.
        unsafe { channel() }.close();
        let mut session = Session::new();
        let mut f = flight();
        for _ in 0..40 {
            session.run(&mut f, &Pad::default(), dt, flat);
        }
        assert_eq!((session.mode, f.tour_on), (Mode::Flight, true));
        session.run(&mut f, &Pad { buttons: pad::MENU, ..Pad::default() }, dt, flat);
        assert!(!f.tour_on);
    }
}
