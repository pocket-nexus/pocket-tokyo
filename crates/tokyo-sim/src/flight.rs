//! A flight over the city as a handheld runs it: the clock, the tour or the
//! pad's own camera, the distances of the levels of detail, and the words a
//! development host steers them with.

use crate::camera::{Camera, Input};
use crate::math::*;
use crate::tour::Tour;
use crate::view::{Governor, Reach};
use alloc::vec::Vec;

/// Buttons a flight reads besides the camera's own (`camera::btn`), as edges and levels of the pad.
pub mod key {
    pub const TOUR: u32 = 1 << 8;
    pub const STATS: u32 = 1 << 9;
    pub const LATER: u32 = 1 << 10;
    pub const EARLIER: u32 = 1 << 11;
    pub const FASTER: u32 = 1 << 12;
    pub const SLOWER: u32 = 1 << 13;
}

pub struct Flight {
    pub cam: Camera,
    pub tour: Tour,
    pub tour_on: bool,
    pub tour_at: f32,
    /// A fixed camera: eye, target, vertical field of view.
    pub view: Option<(V3, V3, f32)>,
    /// Tokyo's clock, and how many hours pass in a second.
    pub hour: f32,
    pub rate: f32,
    pub show: Reach,
    pub governor: Governor,
    pub stats: bool,
    pub traffic: bool,
    /// Display refreshes per frame.
    pub pace: u32,
    pub seconds: f32,
    /// Metres the eye keeps above whatever stands under it and around it.
    pub clearance: f32,
    held: u32,
    /// Metres the tour's eye is lifted above its own path, and the height the free camera keeps above.
    lift: f32,
    floor: f32,
    /// Numbers a device reads for itself: `name=value` words this flight does not know.
    pub extra: Vec<(u32, f32)>,
}

/// A word's name as a number, for `Flight::extra`.
pub const fn name(s: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    let mut i = 0;
    while i < s.len() {
        h = (h ^ s[i] as u32).wrapping_mul(0x0100_0193);
        i += 1;
    }
    h
}

pub fn parse_f32(s: &str) -> Option<f32> {
    let b = s.as_bytes();
    let (neg, mut i) = if b.first() == Some(&b'-') { (true, 1) } else { (false, 0) };
    if i >= b.len() {
        return None;
    }
    let (mut v, mut scale, mut frac) = (0.0f32, 1.0f32, false);
    while i < b.len() {
        match b[i] {
            b'0'..=b'9' => {
                if frac {
                    scale *= 0.1;
                    v += (b[i] - b'0') as f32 * scale;
                } else {
                    v = v * 10.0 + (b[i] - b'0') as f32;
                }
            }
            b'.' if !frac => frac = true,
            _ => return None,
        }
        i += 1;
    }
    Some(if neg { -v } else { v })
}

impl Flight {
    pub fn new(home: [f32; 6], hour: f32, keys: Vec<[f32; 6]>, budget: u32, reach: (f32, f32), pace: u32) -> Flight {
        Flight {
            cam: Camera::looking(v3(home[0], home[1], home[2]), v3(home[3], home[4], home[5])),
            tour: Tour::new(keys, 60.0),
            tour_on: true,
            tour_at: 0.0,
            view: None,
            hour,
            rate: 0.03,
            show: Reach { near: reach.0, mid: reach.1, sectors: true, split: false },
            governor: Governor::new(budget, reach),
            stats: false,
            traffic: true,
            pace,
            seconds: 0.0,
            clearance: 14.0,
            held: u32::MAX,
            lift: -1.0,
            floor: f32::MIN,
            extra: Vec::new(),
        }
    }

    /// A number a device asked for by name, or `fallback`.
    pub fn number(&self, id: u32, fallback: f32) -> f32 {
        self.extra.iter().rev().find(|e| e.0 == id).map(|e| e.1).unwrap_or(fallback)
    }

    /// Words from the development host: `tour=1 restart=1 hour=18.5 rate=0.5 near=200 mid=900 budget=28000
    /// reach=250,1200 sectors=1 stats=1 traffic=1 pace=2 view=x,y,z,tx,ty,tz[,fov] view=off fly=x,y,z,tx,ty,tz`.
    pub fn control(&mut self, text: &str) {
        for word in text.split_ascii_whitespace() {
            let Some((k, v)) = word.split_once('=') else { continue };
            let mut list = [0.0f32; 7];
            let mut count = 0;
            for part in v.split(',') {
                if count < 7 {
                    if let Some(x) = parse_f32(part) {
                        list[count] = x;
                        count += 1;
                    }
                }
            }
            let x = list[0];
            let on = x != 0.0;
            match k {
                "tour" => self.tour_on = on,
                "restart" => self.tour_at = 0.0,
                "at" => self.tour_at = x,
                "hour" => self.hour = x,
                "rate" => self.rate = x,
                "stats" => self.stats = on,
                "traffic" => self.traffic = on,
                "sectors" => self.show.sectors = on,
                "clearance" => self.clearance = x,
                "pace" if count == 1 => self.pace = (x as u32).clamp(1, 6),
                // `near` and `mid` set the distances outright and stop the governor; `budget` hands them back.
                "near" => {
                    self.show.near = x;
                    self.governor.on = false;
                }
                "mid" => {
                    self.show.mid = x;
                    self.governor.on = false;
                }
                "budget" => {
                    self.governor.budget = x as u32;
                    self.governor.on = true;
                }
                "reach" if count == 2 => self.governor.reach = (list[0], list[1]),
                "view" if count >= 6 => self.view = Some((v3(list[0], list[1], list[2]), v3(list[3], list[4], list[5]), if count == 7 { list[6] } else { 55.0 })),
                "view" => self.view = None,
                "fly" if count >= 6 => {
                    self.cam = Camera::looking(v3(list[0], list[1], list[2]), v3(list[3], list[4], list[5]));
                    self.view = None;
                    self.tour_on = false;
                }
                _ if count == 1 => self.extra.push((name(k.as_bytes()), x)),
                _ => {}
            }
        }
    }

    /// One frame of `dt` seconds. `buttons`: `key` bits held; `top(x, z)`: the height of whatever stands at a point.
    pub fn step(&mut self, inp: &Input, buttons: u32, dt: f32, top: impl Fn(f32, f32) -> f32) {
        let pressed = buttons & !self.held;
        self.held = buttons;
        self.seconds += dt;
        if pressed & key::STATS != 0 {
            self.stats = !self.stats;
        }
        if pressed & key::TOUR != 0 && !self.tour.is_empty() {
            self.tour_on = !self.tour_on;
            self.view = None;
        }
        // The clock: turned by hand, or left to run at a rate.
        if buttons & key::LATER != 0 {
            self.hour += 2.5 * dt;
        }
        if buttons & key::EARLIER != 0 {
            self.hour -= 2.5 * dt;
        }
        if pressed & key::FASTER != 0 {
            self.rate = min(self.rate + 0.25, 4.0);
        }
        if pressed & key::SLOWER != 0 {
            self.rate = max(self.rate - 0.25, 0.0);
        }
        self.hour += self.rate * dt;
        self.hour -= floor(self.hour / 24.0) * 24.0;
        // The stick takes the camera off the tour, where it is.
        if self.tour_on && (abs(inp.lx) + abs(inp.ly) + abs(inp.rx) + abs(inp.ry) > 0.2 || inp.buttons != 0) && self.seconds > 1.0 {
            self.tour_on = false;
        }
        // The eye stays above what stands under it and a step around it.
        let clear = self.clearance;
        let around = |x: f32, z: f32| {
            let mut h = top(x, z);
            for (dx, dz) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                h = max(h, top(x + dx * clear, z + dz * clear));
            }
            h
        };
        if self.tour_on && !self.tour.is_empty() {
            self.tour_at += dt;
            let (mut eye, target) = self.tour.at(self.tour_at);
            // What the path must clear, here and over the two seconds ahead: the eye starts to rise before it
            // reaches a tower and comes down after it, in a slope, never a step.
            let mut need = 0.0f32;
            for k in 0..6 {
                let (p, _) = self.tour.at(self.tour_at + k as f32 * 0.4);
                need = max(need, around(p.x, p.z) + clear - p.y);
            }
            self.lift = if self.lift < 0.0 { need } else { ease(self.lift, need, 1.6, dt) };
            eye.y += self.lift;
            self.cam = Camera::looking(eye, target);
            self.floor = f32::MIN;
        } else {
            // The free camera's floor follows what stands around it at a pace, so that a tower ahead lifts the
            // eye over a second and does not throw it.
            let want = around(self.cam.pos.x, self.cam.pos.z) + clear - crate::camera::CLEARANCE;
            self.floor = if self.floor == f32::MIN { want } else { ease(self.floor, want, 3.0, dt) };
            let floor = self.floor;
            self.cam.fly(inp, dt, |_, _| floor);
            self.lift = -1.0;
        }
    }

    /// The eye, the direction it looks in and the vertical field of view, degrees.
    pub fn eye(&self) -> (V3, V3, f32) {
        match self.view {
            Some((pos, target, fov)) => (pos, (target - pos).norm_or(v3(0.0, 0.0, -1.0)), fov),
            None => (self.cam.pos, self.cam.look(), self.cam.fov),
        }
    }

    /// After a frame that drew `drawn` triangles.
    pub fn drew(&mut self, drawn: u32) {
        self.governor.after(drawn, &mut self.show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn words_steer_a_flight() {
        let mut f = Flight::new([0.0, 100.0, 0.0, 0.0, 0.0, -100.0], 12.0, vec![[0.0, 100.0, 0.0, 50.0, 0.0, 0.0], [500.0, 100.0, 0.0, 50.0, 0.0, 0.0]], 28_000, (250.0, 1200.0), 2);
        f.control("hour=18.5 near=120 view=1,2,3,4,5,6,40 light=0.5 nonce=12");
        assert_eq!(f.hour, 18.5);
        assert!(!f.governor.on && f.show.near == 120.0);
        assert_eq!(f.view.map(|v| v.2), Some(40.0));
        assert_eq!(f.number(name(b"light"), 1.0), 0.5);
        f.control("view=off budget=20000");
        assert!(f.view.is_none() && f.governor.on && f.governor.budget == 20000);
        assert_eq!(parse_f32("-12.25"), Some(-12.25));
        assert_eq!(parse_f32("x"), None);
        // The tour carries the eye, above what stands there.
        f.step(&Input::default(), 0, 0.5, |_, _| 300.0);
        assert!(f.tour_on && f.cam.pos.y >= 314.0);
        // A tower that comes up on the path lifts the eye by a slope: no frame moves it more than a few metres.
        let mut last = f.cam.pos.y;
        for _ in 0..240 {
            f.step(&Input::default(), 0, 1.0 / 30.0, |x, _| if x > 200.0 { 520.0 } else { 300.0 });
            assert!((f.cam.pos.y - last).abs() < 12.0, "the eye jumped {} m in a frame", f.cam.pos.y - last);
            last = f.cam.pos.y;
        }
        assert!(last > 500.0);
    }
}
