//! The city as heights: for each cell of a grid, the top of whatever stands there. The device sweeps it into the
//! height below which a point is in shadow, for wherever the sun is.

use crate::geom::P3;

pub struct Heights {
    pub x0: f32,
    pub z0: f32,
    pub step: f32,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Heights {
    pub fn new(x0: f32, z0: f32, x1: f32, z1: f32, step: f32, floor: f32) -> Heights {
        let (w, h) = (((x1 - x0) / step).ceil() as usize, ((z1 - z0) / step).ceil() as usize);
        Heights { x0, z0, step, w, h, data: vec![floor; w * h] }
    }

    /// Raises every cell whose centre the triangle covers to the triangle's height there.
    pub fn raise(&mut self, t: &[P3; 3]) {
        let (a, b, c) = (t[0], t[1], t[2]);
        let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if d.abs() < 1e-9 {
            return;
        }
        let lo = |k: usize| a[k].min(b[k]).min(c[k]);
        let hi = |k: usize| a[k].max(b[k]).max(c[k]);
        let first = |v: f32, o: f32| ((v - o) / self.step - 0.5).ceil().max(0.0) as isize;
        let last = |v: f32, o: f32, n: usize| (((v - o) / self.step - 0.5).floor() as isize).min(n as isize - 1);
        let (i0, j0) = (first(lo(0), self.x0), first(lo(2), self.z0));
        let (i1, j1) = (last(hi(0), self.x0, self.w), last(hi(2), self.z0, self.h));
        for j in j0..=j1 {
            for i in i0..=i1 {
                let (x, z) = (self.x0 + (i as f32 + 0.5) * self.step, self.z0 + (j as f32 + 0.5) * self.step);
                let u = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
                let v = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
                let w = 1.0 - u - v;
                if u >= -1e-4 && v >= -1e-4 && w >= -1e-4 {
                    let y = u * a[1] + v * b[1] + w * c[1];
                    let o = j as usize * self.w + i as usize;
                    if y > self.data[o] {
                        self.data[o] = y;
                    }
                }
            }
        }
    }
}

impl Heights {
    fn at(&self, x: f32, z: f32) -> f32 {
        let i = (((x - self.x0) / self.step) as isize).clamp(0, self.w as isize - 1) as usize;
        let j = (((z - self.z0) / self.step) as isize).clamp(0, self.h as isize - 1) as usize;
        self.data[j * self.w + i]
    }

    /// How much of the sky a point with normal `n` sees, as a vertex stores it: the city's own heights around
    /// the point hide the sky up to the steepest of them, in eight directions.
    pub fn open(&self, p: P3, n: P3) -> u8 {
        const DIRS: [(f32, f32); 8] = [(1.0, 0.0), (0.707, 0.707), (0.0, 1.0), (-0.707, 0.707), (-1.0, 0.0), (-0.707, -0.707), (0.0, -1.0), (0.707, -0.707)];
        // A wall looks from a step in front of itself, and only at the half of the sky it faces.
        let wall = n[1] < 0.5;
        let (x, y, z) = if wall { (p[0] + n[0] * 1.5, p[1] + 0.5, p[2] + n[2] * 1.5) } else { (p[0], p[1] + 0.6, p[2]) };
        let (mut seen, mut count) = (0.0f32, 0.0f32);
        for (dx, dz) in DIRS {
            if wall && dx * n[0] + dz * n[2] < -0.2 {
                continue;
            }
            let mut steepest = 0.0f32;
            for r in [3.0f32, 7.0, 14.0, 28.0] {
                steepest = steepest.max((self.at(x + dx * r, z + dz * r) - y) / r);
            }
            // The sky above an obstacle whose top stands at that slope: 1 - sin of its angle.
            seen += 1.0 - steepest / (1.0 + steepest * steepest).sqrt();
            count += 1.0;
        }
        let open = if count > 0.0 { seen / count } else { 1.0 };
        ((0.3 + 0.7 * open) * 255.0) as u8
    }
}
