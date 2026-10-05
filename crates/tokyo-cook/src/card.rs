//! A lattice tower for a machine that cannot draw its members: a picture of
//! the tower from the side, with holes where the sky shows through, on the
//! four faces of the shape that holds it.
//!
//! The picture is drawn here from the lattice's own triangles, as a camera
//! far to the south sees them. A tower that stands on four legs looks the same
//! from its four sides, so one picture serves the four faces; through the
//! holes of the near faces the far ones show.

use crate::city::{SolidTri, WallTri};
use crate::geom::P3;
use crate::tex::Image;
use std::collections::HashMap;

pub struct Card {
    /// Middle of the tower's plan.
    pub at: [f32; 2],
    /// The faces, with `uv` across and down the picture (`v` in 0..1).
    pub faces: Vec<WallTri>,
    pub day: Image,
    pub night: Image,
}

/// Levels between the foot and the top at which the faces bend.
const BANDS: usize = 14;
const SAMPLES: usize = 4;

fn card(tris: &[&SolidTri]) -> Option<Card> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for t in tris {
        for p in &t.p {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let height = hi[1] - lo[1];
    if height < 40.0 || tris.len() < 200 {
        return None;
    }
    let (cx, cz) = ((lo[0] + hi[0]) * 0.5, (lo[2] + hi[2]) * 0.5);
    let reach = ((hi[0] - lo[0]).max(hi[2] - lo[2]) * 0.5 + 1.0).ceil();
    // ---- how far the tower reaches from its middle, level by level
    let mut wide = [0.0f32; BANDS + 1];
    for t in tris {
        for p in &t.p {
            let level = ((p[1] - lo[1]) / height * BANDS as f32).clamp(0.0, BANDS as f32);
            let w = (p[0] - cx).abs().max((p[2] - cz).abs()) + 0.5;
            for k in [level.floor() as usize, level.ceil() as usize] {
                wide[k] = wide[k].max(w);
            }
        }
    }
    // ---- the picture: the tower from the south, `SAMPLES` squared points per texel
    let (pw, ph) = (if reach * 2.0 / height < 0.36 { 128usize } else { 256 }, 512usize);
    let (sw, sh) = (pw * SAMPLES, ph * SAMPLES);
    let mut depth = vec![f32::NEG_INFINITY; sw * sh];
    let mut color = vec![[0u8; 3]; sw * sh];
    let to = |p: &P3| -> (f32, f32) { ((p[0] - (cx - reach)) / (2.0 * reach) * sw as f32, (hi[1] - p[1]) / height * sh as f32) };
    for t in tris {
        let (a, b, c) = (to(&t.p[0]), to(&t.p[1]), to(&t.p[2]));
        let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        if area.abs() < 1e-6 {
            continue;
        }
        let (x0, x1) = (a.0.min(b.0).min(c.0).floor().max(0.0) as usize, (a.0.max(b.0).max(c.0).ceil() as usize).min(sw - 1));
        let (y0, y1) = (a.1.min(b.1).min(c.1).floor().max(0.0) as usize, (a.1.max(b.1).max(c.1).ceil() as usize).min(sh - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let u = ((b.0 - px) * (c.1 - py) - (c.0 - px) * (b.1 - py)) / area;
                let v = ((c.0 - px) * (a.1 - py) - (a.0 - px) * (c.1 - py)) / area;
                let w = 1.0 - u - v;
                if u < 0.0 || v < 0.0 || w < 0.0 {
                    continue;
                }
                // (south is +z: the nearest to the camera has the largest z)
                let z = u * t.p[0][2] + v * t.p[1][2] + w * t.p[2][2];
                if z > depth[y * sw + x] {
                    depth[y * sw + x] = z;
                    color[y * sw + x] = [t.color[0], t.color[1], t.color[2]];
                }
            }
        }
    }
    let mut day = Image::blank(pw, ph, [0; 3]);
    let mut night = Image::blank(pw, ph, [0; 3]);
    let mut mean = [0u64; 4];
    for y in 0..ph {
        for x in 0..pw {
            let mut sum = [0u32; 3];
            let mut hit = 0u32;
            for j in 0..SAMPLES {
                for i in 0..SAMPLES {
                    let o = (y * SAMPLES + j) * sw + x * SAMPLES + i;
                    if depth[o] > f32::NEG_INFINITY {
                        hit += 1;
                        for k in 0..3 {
                            sum[k] += color[o][k] as u32;
                        }
                    }
                }
            }
            let o = (y * pw + x) * 4;
            if hit > 0 {
                for k in 0..3 {
                    day.rgba[o + k] = (sum[k] / hit) as u8;
                    mean[k] += (sum[k] / hit) as u64;
                }
                mean[3] += 1;
            }
            // A member thinner than a texel still reads as steel.
            day.rgba[o + 3] = ((hit as f32 / (SAMPLES * SAMPLES) as f32 * 1.7).min(1.0) * 255.0) as u8;
        }
    }
    // Where no member is, the colour of the tower itself: a texture filter mixes neighbours, hole or not.
    let fill: [u8; 3] = [0, 1, 2].map(|k| (mean[k] / mean[3].max(1)) as u8);
    for o in (0..pw * ph * 4).step_by(4) {
        if day.rgba[o + 3] == 0 {
            day.rgba[o..o + 3].copy_from_slice(&fill);
        }
        // By night the tower stands in its own floodlight.
        for (k, gain) in [1.55, 1.12, 0.62].into_iter().enumerate() {
            night.rgba[o + k] = (day.rgba[o + k] as f32 * gain + [38.0, 18.0, 4.0][k]).min(255.0) as u8;
        }
        night.rgba[o + 3] = day.rgba[o + 3];
    }
    // ---- four faces, bending at each level
    let y_at = |k: usize| lo[1] + height * k as f32 / BANDS as f32;
    let mut faces = Vec::new();
    // (outward direction, and the direction along the face that has the outside on its right hand)
    for (out, along) in [([0.0f32, 1.0], [-1.0f32, 0.0]), ([0.0, -1.0], [1.0, 0.0]), ([1.0, 0.0], [0.0, 1.0]), ([-1.0, 0.0], [0.0, -1.0])] {
        let n: P3 = [out[0], 0.0, out[1]];
        let corner = |k: usize, side: f32| -> (P3, [f32; 2]) {
            let w = wide[k];
            let p = [cx + out[0] * w + along[0] * w * side, y_at(k), cz + out[1] * w + along[1] * w * side];
            // (across the picture: the place along the face, as the camera in the south sees the south face)
            let across = if out[0] == 0.0 { p[0] - cx } else { p[2] - cz };
            (p, [(across + reach) / (2.0 * reach), (hi[1] - p[1]) / height])
        };
        for k in 0..BANDS {
            if wide[k] <= 0.0 && wide[k + 1] <= 0.0 {
                continue;
            }
            let (a, b, c, d) = (corner(k, -1.0), corner(k, 1.0), corner(k + 1, 1.0), corner(k + 1, -1.0));
            for q in [[a, b, c], [a, c, d]] {
                let mut p = [q[0].0, q[1].0, q[2].0];
                let mut uv = [q[0].1, q[1].1, q[2].1];
                // Counter-clockwise from outside.
                let (g, _) = crate::geom::face(p[0], p[1], p[2]);
                if crate::geom::dot(g, n) < 0.0 {
                    p.swap(1, 2);
                    uv.swap(1, 2);
                }
                faces.push(WallTri { p, n, uv, color: [255; 3], gain: 0, late: 0, sector: crate::city::sector_of(n) });
            }
        }
    }
    Some(Card { at: [cx, cz], faces, day, night })
}

/// A card for each lattice that stands apart from the others.
pub fn cards(lattice: &[SolidTri]) -> Vec<Card> {
    // Triangles that lie within a few squares of the plan of each other belong to one tower.
    const SQUARE: f32 = 48.0;
    let key = |t: &SolidTri| (((t.p[0][0] + t.p[1][0] + t.p[2][0]) / 3.0 / SQUARE).floor() as i32, ((t.p[0][2] + t.p[1][2] + t.p[2][2]) / 3.0 / SQUARE).floor() as i32);
    let mut squares: HashMap<(i32, i32), Vec<&SolidTri>> = HashMap::new();
    for t in lattice {
        squares.entry(key(t)).or_default().push(t);
    }
    let mut keys: Vec<(i32, i32)> = squares.keys().copied().collect();
    keys.sort_unstable();
    let mut seen: HashMap<(i32, i32), bool> = HashMap::new();
    let mut out = Vec::new();
    for start in keys {
        if seen.contains_key(&start) {
            continue;
        }
        let mut group: Vec<&SolidTri> = Vec::new();
        let mut open = vec![start];
        seen.insert(start, true);
        while let Some(k) = open.pop() {
            group.extend(squares[&k].iter().copied());
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let n = (k.0 + dx, k.1 + dz);
                    if squares.contains_key(&n) && seen.insert(n, true).is_none() {
                        open.push(n);
                    }
                }
            }
        }
        out.extend(card(&group));
    }
    out
}
