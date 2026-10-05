//! Landmarks. The export records a landmark as what its model is made of
//! (`web/src/world/landmark.js`): beams with a rank, boxes. A machine gets a
//! model of it made from those members, at three levels of detail:
//!
//! - near: every member. A beam is a ribbon, two triangles seen from both
//!   sides, as wide as the beam is thick; a beam that carries the outline
//!   (rank 0) is two ribbons crossed, so it has a body from every side.
//! - mid: the frame without its bracing (ranks 0 and 1), no thinner than a
//!   width that still covers a pixel at that distance.
//! - far: the outline alone (rank 0), wider still.
//!
//! Boxes (decks, masts) are in all three. No level is a simplification of
//! another: each is built from the members.

use crate::city::SolidTri;
use crate::geom::{cross, dot, face, norm, srgb8, sub, P3};
use crate::ir::Bundle;
use serde_json::Value;

pub struct Beam {
    pub p: P3,
    pub q: P3,
    pub thick: f32,
    pub color: [u8; 4],
    pub rank: u8,
}

pub struct Prism {
    pub ring: [[f32; 2]; 4],
    pub y0: f32,
    pub y1: f32,
    pub color: [u8; 4],
}

pub struct Landmark {
    pub name: String,
    pub at: [f32; 2],
    pub beams: Vec<Beam>,
    pub boxes: Vec<Prism>,
}

/// The landmarks of an export; none when the export has no record of them.
pub fn read(path: &std::path::Path) -> Result<Vec<Landmark>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let b = Bundle::read(path)?;
    let (beams, boxes) = (b.f32("beams"), b.f32("boxes"));
    let color = |c: &[f32]| [srgb8(c[0]), srgb8(c[1]), srgb8(c[2]), 0];
    let range = |v: &Value| (v[0].as_u64().unwrap_or(0) as usize, v[1].as_u64().unwrap_or(0) as usize);
    Ok(b.meta["landmarks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|l| {
            let (first, count) = range(&l["beams"]);
            let (box_first, box_count) = range(&l["boxes"]);
            Landmark {
                name: l["model"].as_str().unwrap_or("landmark").to_string(),
                at: [l["x"].as_f64().unwrap_or(0.0) as f32, l["z"].as_f64().unwrap_or(0.0) as f32],
                beams: beams[first * 11..(first + count) * 11].chunks_exact(11).map(|m| Beam { p: [m[0], m[1], m[2]], q: [m[3], m[4], m[5]], thick: m[6], color: color(&m[7..10]), rank: m[10] as u8 }).collect(),
                boxes: boxes[box_first * 13..(box_first + box_count) * 13].chunks_exact(13).map(|m| Prism { ring: [[m[0], m[1]], [m[2], m[3]], [m[4], m[5]], [m[6], m[7]]], y0: m[8], y1: m[9], color: color(&m[10..13]) }).collect(),
            }
        })
        .collect())
}

/// How a level of detail is made: the ranks it keeps, and the least width of a ribbon.
#[derive(Clone, Copy)]
pub struct Rule {
    pub rank: u8,
    pub thick: f32,
}

/// A quad as two triangles whose faces look the way `out` points.
fn quad(to: &mut Vec<SolidTri>, mut c: [P3; 4], out: P3, color: [u8; 4]) {
    let (n, _) = face(c[0], c[1], c[2]);
    if dot(n, out) < 0.0 {
        c.swap(1, 3);
    }
    let (n, area) = face(c[0], c[1], c[2]);
    if area < 1e-6 {
        return;
    }
    to.push(SolidTri { p: [c[0], c[1], c[2]], n: [n; 3], color });
    to.push(SolidTri { p: [c[0], c[2], c[3]], n: [n; 3], color });
}

/// The landmark at one level of detail: triangles to be seen from both sides.
pub fn model(l: &Landmark, rule: Rule, crossed: bool) -> Vec<SolidTri> {
    let mut out = Vec::new();
    for b in l.beams.iter().filter(|b| b.rank <= rule.rank) {
        let d = norm(sub(b.q, b.p));
        // Outward from the landmark's axis, square to the beam: the ribbon lies across that.
        let mid = [(b.p[0] + b.q[0]) * 0.5 - l.at[0], 0.0, (b.p[2] + b.q[2]) * 0.5 - l.at[1]];
        let lean = dot(mid, d);
        let mut away = [mid[0] - d[0] * lean, mid[1] - d[1] * lean, mid[2] - d[2] * lean];
        if dot(away, away) < 0.04 {
            // (a beam that points at the axis: any side does)
            away = cross(d, if d[1].abs() > 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] });
        }
        let away = norm(away);
        let side = cross(d, away);
        let half = b.thick.max(rule.thick) * 0.5;
        let ribbon = |w: P3| -> [P3; 4] { [[b.p[0] - w[0] * half, b.p[1] - w[1] * half, b.p[2] - w[2] * half], [b.q[0] - w[0] * half, b.q[1] - w[1] * half, b.q[2] - w[2] * half], [b.q[0] + w[0] * half, b.q[1] + w[1] * half, b.q[2] + w[2] * half], [b.p[0] + w[0] * half, b.p[1] + w[1] * half, b.p[2] + w[2] * half]] };
        quad(&mut out, ribbon(side), away, b.color);
        if crossed && b.rank == 0 {
            // (the second ribbon stands on the first; it is lit as what faces sideways from here)
            quad(&mut out, ribbon(away), side, b.color);
        }
    }
    for b in &l.boxes {
        let (cx, cz) = (b.ring.iter().map(|c| c[0]).sum::<f32>() / 4.0, b.ring.iter().map(|c| c[1]).sum::<f32>() / 4.0);
        for i in 0..4 {
            let (a, c) = (b.ring[i], b.ring[(i + 1) % 4]);
            quad(&mut out, [[a[0], b.y0, a[1]], [c[0], b.y0, c[1]], [c[0], b.y1, c[1]], [a[0], b.y1, a[1]]], [(a[0] + c[0]) * 0.5 - cx, 0.0, (a[1] + c[1]) * 0.5 - cz], b.color);
        }
        let top = |y: f32| -> [P3; 4] { [0, 1, 2, 3].map(|i| [b.ring[i][0], y, b.ring[i][1]]) };
        quad(&mut out, top(b.y1), [0.0, 1.0, 0.0], b.color);
        quad(&mut out, top(b.y0), [0.0, -1.0, 0.0], b.color);
    }
    out
}
