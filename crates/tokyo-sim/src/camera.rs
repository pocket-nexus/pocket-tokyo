//! The camera: flown freely over the city, or carried along a tour.

use crate::math::*;

pub mod btn {
    /// Fly faster.
    pub const FAST: u32 = 1;
    pub const UP: u32 = 2;
    pub const DOWN: u32 = 4;
}

/// The pad as the camera reads it: sticks in -1..1 (forward and right positive), and `btn` bits.
#[derive(Clone, Copy, Default, Debug)]
pub struct Input {
    pub buttons: u32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub pos: V3,
    pub yaw: f32,
    pub pitch: f32,
    /// Vertical field of view, degrees.
    pub fov: f32,
    vel: V3,
    turn: (f32, f32),
}

/// Metres the eye keeps above whatever is under it.
pub const CLEARANCE: f32 = 6.0;
pub const CEILING: f32 = 1400.0;

impl Camera {
    pub fn looking(pos: V3, target: V3) -> Camera {
        let d = (target - pos).norm_or(v3(0.0, 0.0, -1.0));
        Camera { pos, yaw: yaw_of(d), pitch: asin(d.y), fov: 55.0, vel: V3::ZERO, turn: (0.0, 0.0) }
    }

    pub fn look(&self) -> V3 {
        forward(self.yaw, self.pitch)
    }

    /// One step of free flight. `top(x, z)`: the height of whatever stands at a point.
    pub fn fly(&mut self, inp: &Input, dt: f32, top: impl Fn(f32, f32) -> f32) {
        // Turning eases in and out, so a flick of the stick does not jerk the picture.
        let want = (-inp.rx * 1.9, inp.ry * 1.3);
        self.turn.0 = ease(self.turn.0, want.0, 9.0, dt);
        self.turn.1 = ease(self.turn.1, want.1, 9.0, dt);
        self.yaw = wrap_angle(self.yaw + self.turn.0 * dt);
        self.pitch = clamp(self.pitch + self.turn.1 * dt, -1.5, 1.2);
        // The speed grows with the height above the city: metres at street level, a district a second from high up.
        let floor = top(self.pos.x, self.pos.z);
        let above = max(self.pos.y - floor, 0.0);
        let pace = clamp(8.0 + above * 0.8, 10.0, 260.0) * if inp.buttons & btn::FAST != 0 { 2.5 } else { 1.0 };
        let ahead = self.look();
        let right = ahead.cross(V3::UP).norm_or(v3(1.0, 0.0, 0.0));
        let lift = (if inp.buttons & btn::UP != 0 { 1.0 } else { 0.0 }) - (if inp.buttons & btn::DOWN != 0 { 1.0 } else { 0.0 });
        let want = (ahead * inp.ly + right * inp.lx) * pace + V3::UP * (lift * pace * 0.7);
        self.vel = self.vel.ease(want, 5.0, dt);
        self.pos += self.vel * dt;
        let floor = top(self.pos.x, self.pos.z) + CLEARANCE;
        if self.pos.y < floor {
            self.pos.y = ease(self.pos.y, floor, 14.0, dt).max(floor - 2.0);
            self.vel.y = max(self.vel.y, 0.0);
        }
        self.pos.y = min(self.pos.y, CEILING);
    }
}
