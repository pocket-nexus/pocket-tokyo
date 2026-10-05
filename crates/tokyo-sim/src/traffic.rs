//! Cars on the road graph. A lane is a line of points from one junction to
//! another, already laid on its side of the road; a car runs along its lane
//! at the lane's speed and takes one of the lanes that leave the junction at
//! its end. Cars do not see each other: from the air a street's worth of them
//! reads as traffic.

use crate::math::*;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Lane {
    /// Its points in the point list.
    pub first: u32,
    pub count: u32,
    /// The junctions it leaves and reaches.
    pub from: u32,
    pub to: u32,
    /// Metres a second.
    pub speed: f32,
    pub length: f32,
    /// How much traffic it carries, against the other lanes.
    pub weight: f32,
    pub pad: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Car {
    pub lane: u32,
    /// Metres along the lane, and the segment that holds them: its index and where it starts.
    pub s: f32,
    seg: u32,
    seg_at: f32,
    /// Its own pace against the lane's speed.
    pub pace: f32,
    /// A number that stays with the car, for its colour.
    pub look: u32,
    pub pos: V3,
    pub dir: V3,
}

pub struct Traffic {
    lanes: Vec<Lane>,
    points: Vec<[f32; 3]>,
    /// For each junction, where its leaving lanes start in `out`; one more entry closes the last.
    out_first: Vec<u32>,
    out: Vec<u32>,
    pub cars: Vec<Car>,
    rng: u32,
}

fn hash(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

impl Traffic {
    pub fn new(lanes: Vec<Lane>, points: Vec<[f32; 3]>, count: usize, seed: u32) -> Traffic {
        let nodes = lanes.iter().map(|l| l.from.max(l.to) + 1).max().unwrap_or(0) as usize;
        let mut out_first = alloc::vec![0u32; nodes + 1];
        for l in &lanes {
            out_first[l.from as usize + 1] += 1;
        }
        for i in 0..nodes {
            out_first[i + 1] += out_first[i];
        }
        let mut fill = out_first.clone();
        let mut out = alloc::vec![0u32; lanes.len()];
        for (i, l) in lanes.iter().enumerate() {
            out[fill[l.from as usize] as usize] = i as u32;
            fill[l.from as usize] += 1;
        }
        let mut t = Traffic { lanes, points, out_first, out, cars: Vec::with_capacity(count), rng: seed | 1 };
        // Cars are spread over the lanes by length and weight.
        let total: f32 = t.lanes.iter().map(|l| l.length * l.weight).sum();
        if total > 0.0 {
            for i in 0..count {
                let mut pick = t.random() * total;
                let mut lane = 0;
                for (k, l) in t.lanes.iter().enumerate() {
                    lane = k;
                    pick -= l.length * l.weight;
                    if pick <= 0.0 {
                        break;
                    }
                }
                let s = t.random() * t.lanes[lane].length;
                let mut car = Car { lane: lane as u32, s, pace: 0.8 + 0.4 * t.random(), look: hash(i as u32 + 77), ..Default::default() };
                t.place(&mut car);
                t.cars.push(car);
            }
        }
        t
    }

    fn random(&mut self) -> f32 {
        self.rng = hash(self.rng.wrapping_add(0x9e37_79b9));
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    fn point(&self, i: u32) -> V3 {
        let p = self.points[i as usize];
        v3(p[0], p[1], p[2])
    }

    /// Finds the car's segment from where it last was, and its place and heading on it.
    fn place(&self, car: &mut Car) {
        let lane = &self.lanes[car.lane as usize];
        loop {
            let (a, b) = (self.point(lane.first + car.seg), self.point(lane.first + car.seg + 1));
            let len = (b - a).len();
            if car.s <= car.seg_at + len || car.seg + 2 >= lane.count {
                let t = if len > 1e-4 { clamp((car.s - car.seg_at) / len, 0.0, 1.0) } else { 0.0 };
                car.pos = a.lerp(b, t);
                car.dir = (b - a).norm_or(car.dir);
                return;
            }
            car.seg += 1;
            car.seg_at += len;
        }
    }

    /// Moves every car by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        for i in 0..self.cars.len() {
            let mut car = self.cars[i];
            let lane = self.lanes[car.lane as usize];
            car.s += lane.speed * car.pace * dt;
            if car.s >= lane.length {
                // The junction: one of the lanes that leave it, not straight back where the car came from if another does.
                let (first, end) = (self.out_first[lane.to as usize] as usize, self.out_first[lane.to as usize + 1] as usize);
                let onward: usize = (first..end).filter(|&k| self.lanes[self.out[k] as usize].to != lane.from).count();
                car.look = hash(car.look);
                let next = if onward > 0 {
                    let pick = car.look as usize % onward;
                    (first..end).filter(|&k| self.lanes[self.out[k] as usize].to != lane.from).nth(pick).map(|k| self.out[k])
                } else if end > first {
                    Some(self.out[first + car.look as usize % (end - first)])
                } else {
                    None
                };
                match next {
                    Some(n) => {
                        car.s -= lane.length;
                        car.lane = n;
                    }
                    None => {
                        // A road that leaves the area: the car comes in again somewhere else.
                        car.lane = car.look % self.lanes.len() as u32;
                        car.s = 0.0;
                    }
                }
                car.seg = 0;
                car.seg_at = 0.0;
            }
            self.place(&mut car);
            self.cars[i] = car;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_car_follows_its_lane_and_turns_at_the_junction() {
        // Two lanes end to end and one back: 0 -> 1 -> 2, and 2 -> 0.
        let points = vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 0.0, 100.0], [100.0, 0.0, 100.0], [0.0, 0.0, 0.0]];
        let lane = |first, from, to, length| Lane { first, count: 2, from, to, speed: 10.0, length, weight: 1.0, pad: 0 };
        let mut t = Traffic::new(vec![lane(0, 0, 1, 100.0), lane(2, 1, 2, 100.0), lane(4, 2, 0, 141.42)], points, 0, 1);
        let mut car = Car { lane: 0, s: 95.0, pace: 1.0, ..Default::default() };
        t.place(&mut car);
        t.cars.push(car);
        assert!((t.cars[0].pos - v3(95.0, 0.0, 0.0)).len() < 1e-3);
        t.step(1.0);
        assert_eq!(t.cars[0].lane, 1);
        assert!((t.cars[0].pos - v3(100.0, 0.0, 5.0)).len() < 1e-3);
        assert!(t.cars[0].dir.z > 0.99);
    }
}
