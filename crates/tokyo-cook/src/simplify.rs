//! Painted geometry with fewer triangles. Pieces that hang together are
//! found first: a piece smaller than the level cares about (a bench, a road
//! stud, a lane line on a deck) is left out whole. What remains has its edges
//! collapsed while the surface stays within a distance of where it was, and a
//! colour stays where it is.

use crate::city::SolidTri;
use crate::geom::face;
use std::collections::HashMap;

fn find(parent: &mut [u32], mut a: u32) -> u32 {
    while parent[a as usize] != a {
        parent[a as usize] = parent[parent[a as usize] as usize];
        a = parent[a as usize];
    }
    a
}

/// `tris` without the pieces whose longest side is under `min_size` metres, and within `error` metres of the
/// rest. What shines at night (alpha above 0: lamps, the floodlit tower) is kept as it is.
pub fn solids(tris: &[SolidTri], error: f32, min_size: f32) -> Vec<SolidTri> {
    let (lamps, rest): (Vec<&SolidTri>, Vec<&SolidTri>) = tris.iter().partition(|t| t.color[3] > 0);
    let mut out: Vec<SolidTri> = lamps.into_iter().copied().collect();
    if rest.is_empty() {
        return out;
    }
    // ---- pieces: triangles that share a corner
    let mut corner: HashMap<[i32; 3], u32> = HashMap::new();
    let mut parent: Vec<u32> = Vec::new();
    let mut of: Vec<u32> = Vec::with_capacity(rest.len());
    for t in &rest {
        let ids = t.p.map(|p| {
            let next = parent.len() as u32;
            let i = *corner.entry(p.map(|v| (v * 100.0).round() as i32)).or_insert(next);
            if i == next {
                parent.push(next);
            }
            i
        });
        let (a, b, c) = (find(&mut parent, ids[0]), find(&mut parent, ids[1]), find(&mut parent, ids[2]));
        parent[b as usize] = a;
        parent[c as usize] = a;
        of.push(ids[0]);
    }
    let mut boxes: HashMap<u32, ([f32; 3], [f32; 3])> = HashMap::new();
    for (t, &v) in rest.iter().zip(&of) {
        let b = boxes.entry(find(&mut parent, v)).or_insert(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]));
        for p in &t.p {
            for k in 0..3 {
                b.0[k] = b.0[k].min(p[k]);
                b.1[k] = b.1[k].max(p[k]);
            }
        }
    }
    let kept: Vec<&SolidTri> = rest
        .iter()
        .zip(&of)
        .filter(|(_, &v)| {
            let b = boxes[&find(&mut parent, v)];
            (b.1[0] - b.0[0]).max(b.1[1] - b.0[1]).max(b.1[2] - b.0[2]) >= min_size
        })
        .map(|(t, _)| *t)
        .collect();
    if kept.len() < 32 || error <= 0.0 {
        out.extend(kept.into_iter().copied());
        return out;
    }

    // ---- edge collapse, with a vertex per position and colour
    let mut seen: HashMap<([i32; 3], [u8; 4]), u32> = HashMap::new();
    let mut pos: Vec<f32> = Vec::new();
    let mut col: Vec<[u8; 4]> = Vec::new();
    let mut idx: Vec<u32> = Vec::with_capacity(kept.len() * 3);
    for t in &kept {
        for p in &t.p {
            let key = (p.map(|v| (v * 100.0).round() as i32), t.color);
            let next = col.len() as u32;
            let i = *seen.entry(key).or_insert(next);
            if i == next {
                pos.extend_from_slice(p);
                col.push(t.color);
            }
            idx.push(i);
        }
    }
    let bytes: Vec<u8> = pos.iter().flat_map(|f| f.to_le_bytes()).collect();
    let Ok(adapter) = meshopt::VertexDataAdapter::new(&bytes, 12, 0) else {
        out.extend(kept.into_iter().copied());
        return out;
    };
    let attrs: Vec<f32> = col.iter().flat_map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]).collect();
    let locked = vec![false; col.len()];
    let simple = meshopt::simplify_with_attributes_and_locks(&idx, &adapter, &attrs, &[1.0, 1.0, 1.0], 12, &locked, 0, error, meshopt::SimplifyOptions::ErrorAbsolute, None);
    for t in simple.chunks_exact(3) {
        let p = [0, 1, 2].map(|k| {
            let i = t[k] as usize * 3;
            [pos[i], pos[i + 1], pos[i + 2]]
        });
        let (n, area) = face(p[0], p[1], p[2]);
        if area > 1e-5 {
            out.push(SolidTri { p, n: [n; 3], color: col[t[0] as usize] });
        }
    }
    out
}
