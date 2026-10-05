//! A tour of the city: the camera carried through a closed loop of places,
//! each a point to be at and a point to look at. Between places both points
//! follow a Catmull-Rom curve, at a steady speed along the loop.

use crate::math::*;
use alloc::vec::Vec;

pub struct Tour {
    keys: Vec<[f32; 6]>,
    /// Seconds at which each key is reached; the last entry is the loop's length.
    at: Vec<f32>,
}

fn point(k: &[f32; 6], from: usize) -> V3 {
    v3(k[from], k[from + 1], k[from + 2])
}

fn curve(a: V3, b: V3, c: V3, d: V3, t: f32) -> V3 {
    let (t2, t3) = (t * t, t * t * t);
    (b * 2.0 + (c - a) * t + (a * 2.0 - b * 5.0 + c * 4.0 - d) * t2 + (b * 3.0 - a - c * 3.0 + d) * t3) * 0.5
}

impl Tour {
    /// `keys`: eye x, y, z and target x, y, z per place; `speed`: metres a second.
    pub fn new(keys: Vec<[f32; 6]>, speed: f32) -> Tour {
        let n = keys.len();
        let mut at = Vec::with_capacity(n + 1);
        let mut t = 0.0;
        for i in 0..n {
            at.push(t);
            t += max((point(&keys[(i + 1) % n], 0) - point(&keys[i], 0)).len() / speed, 1.0);
        }
        at.push(t);
        Tour { keys, at }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.len() < 2
    }

    /// Seconds the loop takes.
    pub fn seconds(&self) -> f32 {
        *self.at.last().unwrap_or(&0.0)
    }

    /// The eye and the point looked at, `seconds` into the tour.
    pub fn at(&self, seconds: f32) -> (V3, V3) {
        let n = self.keys.len();
        let t = seconds - floor(seconds / self.seconds()) * self.seconds();
        let mut i = 0;
        while i + 1 < n && t >= self.at[i + 1] {
            i += 1;
        }
        let u = (t - self.at[i]) / (self.at[i + 1] - self.at[i]);
        let k = |d: usize| &self.keys[(i + n + d - 1) % n];
        (curve(point(k(0), 0), point(k(1), 0), point(k(2), 0), point(k(3), 0), u), curve(point(k(0), 3), point(k(1), 3), point(k(2), 3), point(k(3), 3), u))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_tour_passes_through_its_places_and_closes() {
        let tour = Tour::new(vec![[0.0, 10.0, 0.0, 5.0, 0.0, 5.0], [100.0, 10.0, 0.0, 5.0, 0.0, 5.0], [100.0, 10.0, 100.0, 5.0, 0.0, 5.0], [0.0, 10.0, 100.0, 5.0, 0.0, 5.0]], 50.0);
        assert!((tour.seconds() - 8.0).abs() < 1e-4);
        let (eye, target) = tour.at(2.0);
        assert!((eye - v3(100.0, 10.0, 0.0)).len() < 1e-3 && (target - v3(5.0, 0.0, 5.0)).len() < 1e-3);
        let (again, _) = tour.at(8.0 + 2.0);
        assert!((again - eye).len() < 1e-3);
        let (between, _) = tour.at(1.0);
        assert!(between.x > 30.0 && between.x < 70.0);
    }
}
