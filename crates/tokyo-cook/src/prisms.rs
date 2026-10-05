//! The simple levels of detail of the buildings: prisms.
//!
//! Every roof of the area is drawn into one grid of heights. In a tile, cells
//! next to each other at about one height form a terrace, whoever's roof they
//! are; a terrace becomes a prism: its outline, straightened, as walls under
//! the facade pictures, and a flat top under the tile's picture from above.
//! A coarser grid and a wider tolerance give the far level; buildings lower
//! than the level's floor are left to the picture from above.

use crate::buildings::{Building, Facade, Walls, BAY, WV};
use crate::city::{TopTri, WallTri};
use crate::geom::{face, P3};
use crate::ir::Grid;
use std::collections::HashMap;

/// Roof heights of the whole area, and whose roof each cell is.
pub struct RoofGrid {
    pub x0: f32,
    pub z0: f32,
    pub step: f32,
    pub w: usize,
    pub h: usize,
    /// `NEG_INFINITY` where no roof is.
    pub height: Vec<f32>,
    /// Building index + 1 in the area's list; 0 where none is known.
    pub owner: Vec<u32>,
}

impl RoofGrid {
    pub fn new(x0: f32, z0: f32, x1: f32, z1: f32, step: f32) -> RoofGrid {
        let (w, h) = (((x1 - x0) / step).round() as usize, ((z1 - z0) / step).round() as usize);
        RoofGrid { x0, z0, step, w, h, height: vec![f32::NEG_INFINITY; w * h], owner: vec![0; w * h] }
    }

    fn cells(&self, lo: f32, hi: f32, origin: f32, n: usize) -> (isize, isize) {
        ((((lo - origin) / self.step - 0.5).ceil().max(0.0)) as isize, (((hi - origin) / self.step - 0.5).floor() as isize).min(n as isize - 1))
    }

    /// Raises every cell whose centre the triangle covers to the triangle's height there.
    pub fn raise(&mut self, t: &[P3; 3], owner: u32) {
        let (a, b, c) = (t[0], t[1], t[2]);
        let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if d.abs() < 1e-9 {
            return;
        }
        let (i0, i1) = self.cells(a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]), self.x0, self.w);
        let (j0, j1) = self.cells(a[2].min(b[2]).min(c[2]), a[2].max(b[2]).max(c[2]), self.z0, self.h);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let (x, z) = (self.x0 + (i as f32 + 0.5) * self.step, self.z0 + (j as f32 + 0.5) * self.step);
                let u = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
                let v = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
                let w = 1.0 - u - v;
                if u >= -1e-4 && v >= -1e-4 && w >= -1e-4 {
                    let y = u * a[1] + v * b[1] + w * c[1];
                    let o = j as usize * self.w + i as usize;
                    if y > self.height[o] {
                        self.height[o] = y;
                        if owner != 0 {
                            self.owner[o] = owner;
                        }
                    }
                }
            }
        }
    }

    /// Marks the cells inside a footprint (rings of x, z; even-odd) as `owner`'s where no roof has claimed them,
    /// and gives them the height `top` where no roof was drawn at all.
    pub fn footprint(&mut self, rings: &[&[f32]], owner: u32, top: f32) {
        let (mut x0, mut x1, mut z0, mut z1) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for r in rings {
            for p in r.chunks_exact(2) {
                x0 = x0.min(p[0]);
                x1 = x1.max(p[0]);
                z0 = z0.min(p[1]);
                z1 = z1.max(p[1]);
            }
        }
        if x0 > x1 {
            return;
        }
        let (i0, i1) = self.cells(x0, x1, self.x0, self.w);
        let (j0, j1) = self.cells(z0, z1, self.z0, self.h);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let (x, z) = (self.x0 + (i as f32 + 0.5) * self.step, self.z0 + (j as f32 + 0.5) * self.step);
                let mut inside = false;
                for r in rings {
                    let n = r.len() / 2;
                    let mut k = n - 1;
                    for m in 0..n {
                        let (xi, zi, xk, zk) = (r[m * 2], r[m * 2 + 1], r[k * 2], r[k * 2 + 1]);
                        if (zi > z) != (zk > z) && x < (xk - xi) * (z - zi) / (zk - zi) + xi {
                            inside = !inside;
                        }
                        k = m;
                    }
                }
                if inside {
                    let o = j as usize * self.w + i as usize;
                    if self.owner[o] == 0 {
                        self.owner[o] = owner;
                    }
                    if self.height[o] == f32::NEG_INFINITY {
                        self.height[o] = top;
                    }
                }
            }
        }
    }

    /// The grid at `k` times the cell size: each cell the highest of the cells it covers.
    pub fn coarser(&self, k: usize) -> RoofGrid {
        let (w, h) = (self.w / k, self.h / k);
        let mut g = RoofGrid { x0: self.x0, z0: self.z0, step: self.step * k as f32, w, h, height: vec![f32::NEG_INFINITY; w * h], owner: vec![0; w * h] };
        for j in 0..h {
            for i in 0..w {
                // A cell is roof when at least half of what it covers is.
                let mut roofed = 0;
                let (mut top, mut owner) = (f32::NEG_INFINITY, 0);
                for jj in 0..k {
                    for ii in 0..k {
                        let o = (j * k + jj) * self.w + i * k + ii;
                        if self.height[o] > f32::NEG_INFINITY {
                            roofed += 1;
                            if self.height[o] > top {
                                top = self.height[o];
                                owner = self.owner[o];
                            }
                        }
                    }
                }
                if roofed * 2 >= k * k {
                    g.height[j * w + i] = top;
                    g.owner[j * w + i] = owner;
                }
            }
        }
        g
    }
}

pub struct Rules {
    /// Metres of height one terrace may span.
    pub cap: f32,
    /// A terrace smaller than this (m²) joins a neighbour, or is left out.
    pub min_area: f32,
    /// How far (m) a straightened outline may leave the cells' own.
    pub eps: f32,
    /// Roofs lower than this above the ground are left to the picture from above.
    pub min_height: f32,
    /// Passes of the smoothing of the terraces' outlines.
    pub smooth: u32,
}

const NONE: u32 = u32::MAX;

/// Counters of a run, for the receipt: terraces, cell edges of their outlines, edges after straightening, walls.
pub static COUNT: [std::sync::atomic::AtomicUsize; 4] = [const { std::sync::atomic::AtomicUsize::new(0) }; 4];
fn count(k: usize, n: usize) {
    COUNT[k].fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

#[derive(Clone, Copy)]
struct Edge {
    /// Start and end vertex, as `j * (n + 1) + i` on the tile's lattice of cell corners.
    from: u32,
    to: u32,
    /// What lies across: `NONE`, a region, or (`open`) the same region in the next tile.
    across: u32,
    open: bool,
}

/// Ramer-Douglas-Peucker over `pts[a..=b]`: marks the points to keep.
fn rdp(pts: &[[f32; 2]], a: usize, b: usize, eps: f32, keep: &mut [bool]) {
    if b <= a + 1 {
        return;
    }
    let (p, q) = (pts[a], pts[b]);
    let (dx, dz) = (q[0] - p[0], q[1] - p[1]);
    let len = (dx * dx + dz * dz).sqrt().max(1e-6);
    let (mut worst, mut at) = (0.0f32, a);
    for (k, c) in pts.iter().enumerate().take(b).skip(a + 1) {
        let d = ((c[0] - p[0]) * dz - (c[1] - p[1]) * dx).abs() / len;
        if d > worst {
            worst = d;
            at = k;
        }
    }
    if worst > eps {
        keep[at] = true;
        rdp(pts, a, at, eps, keep);
        rdp(pts, at, b, eps, keep);
    }
}

fn inside(poly: &[[f32; 2]], x: f32, z: f32) -> bool {
    let mut c = false;
    let mut k = poly.len() - 1;
    for m in 0..poly.len() {
        let (a, b) = (poly[m], poly[k]);
        if (a[1] > z) != (b[1] > z) && x < (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        k = m;
    }
    c
}

pub struct Prisms {
    pub tops: Vec<TopTri>,
    pub walls: Vec<WallTri>,
}

/// The prisms of the tile whose corner is `(x0, z0)`.
#[allow(clippy::too_many_arguments)]
pub fn prisms(grid: &RoofGrid, ground: &Grid, x0: f32, z0: f32, tile: f32, rules: &Rules, buildings: &[Building], facade: &Facade) -> Prisms {
    let n = (tile / grid.step).round() as usize;
    let (gi, gj) = (((x0 - grid.x0) / grid.step).round() as isize, ((z0 - grid.z0) / grid.step).round() as isize);
    // The tile's cells with one more all round: what the neighbours hold along the border.
    let w = n + 2;
    let at = |i: usize, j: usize| j * w + i;
    let cell = |i: usize, j: usize| -> Option<usize> {
        let (ci, cj) = (gi + i as isize - 1, gj + j as isize - 1);
        (ci >= 0 && cj >= 0 && (ci as usize) < grid.w && (cj as usize) < grid.h).then(|| cj as usize * grid.w + ci as usize)
    };
    let centre = |i: usize, j: usize| (x0 + (i as f32 - 0.5) * grid.step, z0 + (j as f32 - 0.5) * grid.step);
    let mut height = vec![f32::NEG_INFINITY; w * w];
    for j in 0..w {
        for i in 0..w {
            if let Some(c) = cell(i, j) {
                let h = grid.height[c];
                let (x, z) = centre(i, j);
                if h > f32::NEG_INFINITY && h - ground.sample(x, z) >= rules.min_height {
                    height[at(i, j)] = h;
                }
            }
        }
    }

    // ---- terraces: from the highest cell not yet taken, everything joined to it within the cap
    let mut label = vec![NONE; w * w];
    let mut order: Vec<usize> = (0..w * w).filter(|&c| height[c] > f32::NEG_INFINITY).collect();
    order.sort_by(|a, b| height[*b].partial_cmp(&height[*a]).unwrap().then(a.cmp(b)));
    let mut regions: Vec<(f32, usize)> = Vec::new(); // (height, cells)
    let mut stack = Vec::new();
    let mut members: Vec<f32> = Vec::new();
    for &seed in &order {
        if label[seed] != NONE {
            continue;
        }
        let r = regions.len() as u32;
        let floor = height[seed] - rules.cap;
        label[seed] = r;
        stack.push(seed);
        members.clear();
        while let Some(c) = stack.pop() {
            members.push(height[c]);
            let (i, j) = (c % w, c / w);
            for (di, dj) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                let (ni, nj) = (i as isize + di, j as isize + dj);
                if ni < 0 || nj < 0 || ni >= w as isize || nj >= w as isize {
                    continue;
                }
                let m = at(ni as usize, nj as usize);
                if label[m] == NONE && height[m] >= floor {
                    label[m] = r;
                    stack.push(m);
                }
            }
        }
        // The terrace stands at the height most of it has: a hut on a roof does not lift the roof.
        members.sort_by(|a, b| a.partial_cmp(b).unwrap());
        regions.push((members[members.len() * 6 / 10], members.len()));
    }
    // ---- a cell goes over to the terrace most of the eight around it belong to: the outlines lose their frays
    for _ in 0..rules.smooth {
        let before = label.clone();
        let mut votes: Vec<(u32, u8)> = Vec::with_capacity(8);
        for j in 1..w - 1 {
            for i in 1..w - 1 {
                let own = before[at(i, j)];
                votes.clear();
                for (di, dj) in [(-1isize, -1isize), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let l = before[at((i as isize + di) as usize, (j as isize + dj) as usize)];
                    match votes.iter_mut().find(|v| v.0 == l) {
                        Some(v) => v.1 += 1,
                        None => votes.push((l, 1)),
                    }
                }
                let (most, n) = votes.iter().copied().max_by_key(|v| (v.1, std::cmp::Reverse(v.0))).unwrap();
                if most != own && n >= 5 {
                    label[at(i, j)] = most;
                    if own != NONE {
                        regions[own as usize].1 -= 1;
                    }
                    if most != NONE {
                        regions[most as usize].1 += 1;
                    }
                }
            }
        }
    }
    // ---- a terrace too small to draw joins the neighbour it shares most of its edge with, or goes
    let cell_area = grid.step * grid.step;
    let mut small: Vec<u32> = (0..regions.len() as u32).filter(|&r| (regions[r as usize].1 as f32) * cell_area < rules.min_area).collect();
    small.sort_by_key(|&r| regions[r as usize].1);
    for r in small {
        let mut shared: HashMap<u32, usize> = HashMap::new();
        for c in 0..w * w {
            if label[c] != r {
                continue;
            }
            let (i, j) = (c % w, c / w);
            for (di, dj) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                let (ni, nj) = (i as isize + di, j as isize + dj);
                if ni < 0 || nj < 0 || ni >= w as isize || nj >= w as isize {
                    continue;
                }
                let l = label[at(ni as usize, nj as usize)];
                if l != NONE && l != r {
                    *shared.entry(l).or_default() += 1;
                }
            }
        }
        let to = shared.into_iter().max_by_key(|&(l, n)| (n, std::cmp::Reverse(l))).map(|(l, _)| l).unwrap_or(NONE);
        for l in label.iter_mut() {
            if *l == r {
                *l = to;
            }
        }
        if to != NONE {
            regions[to as usize].1 += regions[r as usize].1;
        }
        regions[r as usize].1 = 0;
    }

    // ---- outlines: the edges between a terrace's cells in the tile and anything else, walked with the terrace
    // on the right hand
    let lat = n + 1;
    let mut edges: Vec<Vec<Edge>> = vec![Vec::new(); regions.len()];
    for j in 1..=n {
        for i in 1..=n {
            let r = label[at(i, j)];
            if r == NONE {
                continue;
            }
            let (vi, vj) = (i - 1, j - 1);
            let v = |a: usize, b: usize| (b * lat + a) as u32;
            // (neighbour cell, from, to)
            let sides = [((i, j - 1), v(vi, vj), v(vi + 1, vj)), ((i + 1, j), v(vi + 1, vj), v(vi + 1, vj + 1)), ((i, j + 1), v(vi + 1, vj + 1), v(vi, vj + 1)), ((i - 1, j), v(vi, vj + 1), v(vi, vj))];
            for ((ni, nj), from, to) in sides {
                let across = label[at(ni, nj)];
                let out_of_tile = ni == 0 || nj == 0 || ni == n + 1 || nj == n + 1;
                if across != r || out_of_tile {
                    edges[r as usize].push(Edge { from, to, across, open: across == r });
                }
            }
        }
    }

    let mut out = Prisms { tops: Vec::new(), walls: Vec::new() };
    let point = |v: u32| -> [f32; 2] { [x0 + (v as usize % lat) as f32 * grid.step, z0 + (v as usize / lat) as f32 * grid.step] };
    for (r, list) in edges.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let top = regions[r].0;
        count(0, 1);
        count(1, list.len());
        // Chain the edges into loops. Where two of the terrace's cells touch by a corner, the walk takes the
        // sharper turn, so each loop stays a simple outline.
        let mut next: HashMap<u32, Vec<usize>> = HashMap::new();
        for (k, e) in list.iter().enumerate() {
            next.entry(e.from).or_default().push(k);
        }
        let mut used = vec![false; list.len()];
        let mut loops: Vec<Vec<usize>> = Vec::new();
        for start in 0..list.len() {
            if used[start] {
                continue;
            }
            let mut ring = Vec::new();
            let mut k = start;
            loop {
                used[k] = true;
                ring.push(k);
                let e = list[k];
                let (a, b) = (point(e.from), point(e.to));
                let d = [b[0] - a[0], b[1] - a[1]];
                let pick = next.get(&e.to).and_then(|c| {
                    c.iter().copied().filter(|&m| !used[m]).max_by(|&m1, &m2| {
                        let turn = |m: usize| {
                            let (p, q) = (point(list[m].from), point(list[m].to));
                            d[0] * (q[1] - p[1]) - d[1] * (q[0] - p[0])
                        };
                        turn(m1).partial_cmp(&turn(m2)).unwrap()
                    })
                });
                match pick {
                    Some(m) => k = m,
                    None => break,
                }
            }
            if list[*ring.last().unwrap()].to == list[ring[0]].from && ring.len() >= 4 {
                loops.push(ring);
            }
        }

        // Straighten each loop; a straightened edge remembers which cell edges it stands for.
        struct Outline {
            pts: Vec<[f32; 2]>,
            /// For each point: the first cell edge of the outline edge that starts there, and one past its last.
            spans: Vec<(usize, usize)>,
            ring: Vec<usize>,
            area: f32,
        }
        let mut outlines: Vec<Outline> = Vec::new();
        for ring in loops {
            let pts: Vec<[f32; 2]> = ring.iter().map(|&k| point(list[k].from)).collect();
            let m = pts.len();
            // Two anchors far apart, then each half between them.
            let a = (0..m).min_by(|&p, &q| (pts[p][0] + pts[p][1]).partial_cmp(&(pts[q][0] + pts[q][1])).unwrap()).unwrap();
            let rot: Vec<[f32; 2]> = (0..=m).map(|k| pts[(a + k) % m]).collect();
            let b = (0..m).max_by(|&p, &q| {
                let d = |k: usize| (rot[k][0] - rot[0][0]).powi(2) + (rot[k][1] - rot[0][1]).powi(2);
                d(p).partial_cmp(&d(q)).unwrap()
            });
            let b = b.unwrap();
            let mut keep = vec![false; m + 1];
            keep[0] = true;
            keep[b] = true;
            keep[m] = true;
            rdp(&rot, 0, b, rules.eps, &mut keep);
            rdp(&rot, b, m, rules.eps, &mut keep);
            let kept: Vec<usize> = (0..m).filter(|&k| keep[k]).collect();
            if kept.len() < 3 {
                continue;
            }
            let simple: Vec<[f32; 2]> = kept.iter().map(|&k| rot[k]).collect();
            let spans: Vec<(usize, usize)> = kept.iter().enumerate().map(|(q, &k)| (k, if q + 1 < kept.len() { kept[q + 1] } else { m })).collect();
            let area: f32 = (0..simple.len()).map(|k| { let (p, q) = (simple[k], simple[(k + 1) % simple.len()]); p[0] * q[1] - q[0] * p[1] }).sum::<f32>() * 0.5;
            if area.abs() < 0.5 * cell_area {
                continue;
            }
            let ring: Vec<usize> = (0..m).map(|k| ring[(a + k) % m]).collect();
            outlines.push(Outline { pts: simple, spans, ring, area });
        }

        // ---- the top: each outer outline with the holes that lie in it
        let outers: Vec<usize> = (0..outlines.len()).filter(|&k| outlines[k].area > 0.0).collect();
        for &o in &outers {
            let mut flat: Vec<f64> = outlines[o].pts.iter().flat_map(|p| [p[0] as f64, p[1] as f64]).collect();
            let mut holes = Vec::new();
            for hole in outlines.iter() {
                if hole.area < 0.0 && inside(&outlines[o].pts, hole.pts[0][0], hole.pts[0][1]) && (outers.len() == 1 || !outers.iter().any(|&p| p != o && outlines[p].area < outlines[o].area && inside(&outlines[p].pts, hole.pts[0][0], hole.pts[0][1]))) {
                    holes.push(flat.len() / 2);
                    flat.extend(hole.pts.iter().flat_map(|p| [p[0] as f64, p[1] as f64]));
                }
            }
            if let Ok(tris) = earcutr::earcut(&flat, &holes, 2) {
                for t in tris.chunks_exact(3) {
                    let mut p = [0, 1, 2].map(|k| [flat[t[k] * 2] as f32, top, flat[t[k] * 2 + 1] as f32]);
                    crate::terrain::face_up(&mut p);
                    let (_, area) = face(p[0], p[1], p[2]);
                    if area > 1e-4 {
                        out.tops.push(TopTri { p, n: [[0.0, 1.0, 0.0]; 3] });
                    }
                }
            }
        }

        // ---- the walls: an outline edge needs one where anything it stands for looks out over lower ground
        let mut walls = Walls { f: facade, out: &mut out.walls };
        for outline in &outlines {
            let m = outline.pts.len();
            for k in 0..m {
                let (a, b) = (outline.pts[k], outline.pts[(k + 1) % m]);
                let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
                let len = (dx * dx + dz * dz).sqrt();
                if len < 0.05 {
                    continue;
                }
                let mut bottom = f32::INFINITY;
                let (s0, s1) = outline.spans[k];
                for q in s0..s1 {
                    let e = list[outline.ring[q]];
                    if e.open {
                        continue;
                    }
                    if e.across == NONE {
                        let (p, q2) = (point(e.from), point(e.to));
                        bottom = bottom.min(ground.sample(p[0], p[1]).min(ground.sample(q2[0], q2[1])) - 2.0);
                    } else if regions[e.across as usize].0 < top - 0.3 {
                        bottom = bottom.min(regions[e.across as usize].0 - 0.3);
                    }
                }
                count(2, 1);
                if bottom == f32::INFINITY {
                    continue;
                }
                count(3, 1);
                // (with the terrace on the right of a → b, the wall looks to the left)
                let nrm = [dz / len, 0.0, -dx / len];
                // Whose wall it is: the building under the terrace's cells just inside this edge.
                let mut owner = 0;
                for t in [0.5f32, 0.25, 0.75, 0.05, 0.95] {
                    let p = [a[0] + dx * t - nrm[0] * grid.step * 0.5, a[1] + dz * t - nrm[2] * grid.step * 0.5];
                    let (ci, cj) = (((p[0] - grid.x0) / grid.step) as isize, ((p[1] - grid.z0) / grid.step) as isize);
                    if ci >= 0 && cj >= 0 && (ci as usize) < grid.w && (cj as usize) < grid.h {
                        owner = grid.owner[cj as usize * grid.w + ci as usize];
                        if owner != 0 {
                            break;
                        }
                    }
                }
                let info = if owner != 0 { buildings[owner as usize - 1] } else { Building { base: bottom + 2.0, ..Building::PLAIN } };
                let bays = if len >= 1.8 { (len / BAY[info.cat.clamp(0, 5) as usize]).round().max(1.0) } else { 0.0 };
                let corner = |p: [f32; 2], y: f32, u: f32| -> WV { [p[0], y, p[1], u, y - info.base] };
                let mut quad = [[corner(a, bottom, 0.0), corner(b, bottom, bays), corner(b, top, bays)], [corner(a, bottom, 0.0), corner(b, top, bays), corner(a, top, 0.0)]];
                for t in &mut quad {
                    let (g, _) = face([t[0][0], t[0][1], t[0][2]], [t[1][0], t[1][1], t[1][2]], [t[2][0], t[2][1], t[2][2]]);
                    if g[0] * nrm[0] + g[2] * nrm[2] < 0.0 {
                        t.swap(1, 2);
                    }
                }
                if bays > 0.0 {
                    walls.windows(&info, nrm, &quad, bays, (k as u32) ^ ((a[0] as i32 as u32) << 8));
                } else {
                    for t in &quad {
                        walls.plain(&info, nrm, &t.map(|v| [v[0], v[1], v[2]]));
                    }
                }
            }
        }
    }
    out
}
