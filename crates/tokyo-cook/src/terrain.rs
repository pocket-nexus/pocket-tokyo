//! The ground of a tile as a right-triangulated irregular network (after
//! Evans et al. and mapbox/martini): triangles as large as an error bound
//! allows, with every `stride`-th sample along the tile's edge always a
//! vertex, so two tiles at the same level of detail meet without a gap.

use crate::geom::P3;
use crate::ir::Grid;

/// Samples along a tile's side: `CELLS + 1`.
pub const CELLS: usize = 64;
const SIZE: usize = CELLS + 1;

pub struct Ground {
    /// Height at each sample, row by row along +z.
    pub heights: Vec<f32>,
    errors: Vec<f32>,
    pub x0: f32,
    pub z0: f32,
    pub cell: f32,
}

impl Ground {
    pub fn new(surface: &Grid, x0: f32, z0: f32, tile: f32) -> Ground {
        let cell = tile / CELLS as f32;
        let mut heights = vec![0.0; SIZE * SIZE];
        for j in 0..SIZE {
            for i in 0..SIZE {
                heights[j * SIZE + i] = surface.sample(x0 + i as f32 * cell, z0 + j as f32 * cell);
            }
        }
        Ground { heights, errors: Vec::new(), x0, z0, cell }
    }

    /// Triangles of the ground within `max_error` metres of the samples. `stride`: cells between the vertices kept
    /// along the tile's edge (a power of two).
    pub fn mesh(&mut self, max_error: f32, stride: usize) -> Vec<[P3; 3]> {
        self.errors = vec![0.0; SIZE * SIZE];
        let max = CELLS;
        let tris = CELLS * CELLS * 2 - 2;
        let parents = tris - CELLS * CELLS;
        // Coordinates of every triangle of the implicit binary tree, smallest last.
        let mut coords = vec![[0usize; 4]; tris];
        for (i, c) in coords.iter_mut().enumerate() {
            let mut id = i + 2;
            let (mut ax, mut ay, mut bx, mut by, mut cx, mut cy) = (0, 0, 0, 0, 0, 0);
            if id & 1 == 1 {
                bx = max;
                by = max;
                cx = max;
            } else {
                ax = max;
                ay = max;
                cy = max;
            }
            loop {
                id >>= 1;
                if id <= 1 {
                    break;
                }
                let (mx, my) = ((ax + bx) >> 1, (ay + by) >> 1);
                if id & 1 == 1 {
                    bx = ax;
                    by = ay;
                    ax = cx;
                    ay = cy;
                } else {
                    ax = bx;
                    ay = by;
                    bx = cx;
                    by = cy;
                }
                cx = mx;
                cy = my;
            }
            *c = [ax, ay, bx, by];
        }
        let h = &self.heights;
        for i in (0..tris).rev() {
            let [ax, ay, bx, by] = coords[i];
            let (mx, my) = ((ax + bx) >> 1, (ay + by) >> 1);
            let (cx, cy) = (mx + my - ay, my + ax - mx);
            let mid = my * SIZE + mx;
            let mut e = ((h[ay * SIZE + ax] + h[by * SIZE + bx]) / 2.0 - h[mid]).abs();
            if i < parents {
                let left = ((ay + cy) >> 1) * SIZE + ((ax + cx) >> 1);
                let right = ((by + cy) >> 1) * SIZE + ((bx + cx) >> 1);
                e = e.max(self.errors[left]).max(self.errors[right]);
            }
            // A long edge on the tile's border: split down to the stride and no further, whatever the heights say.
            let on_border = (ax == bx && (ax == 0 || ax == max)) || (ay == by && (ay == 0 || ay == max));
            if on_border {
                let len = ax.abs_diff(bx) + ay.abs_diff(by);
                e = if len > stride { f32::INFINITY } else { 0.0 };
            }
            self.errors[mid] = self.errors[mid].max(e);
            if on_border && e == 0.0 {
                self.errors[mid] = 0.0;
            }
        }
        let mut out = Vec::new();
        self.split(0, 0, max, max, max, 0, max_error, &mut out);
        self.split(max, max, 0, 0, 0, max, max_error, &mut out);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn split(&self, ax: usize, ay: usize, bx: usize, by: usize, cx: usize, cy: usize, max_error: f32, out: &mut Vec<[P3; 3]>) {
        let (mx, my) = ((ax + bx) >> 1, (ay + by) >> 1);
        if ax.abs_diff(cx) + ay.abs_diff(cy) > 1 && self.errors[my * SIZE + mx] > max_error {
            self.split(cx, cy, ax, ay, mx, my, max_error, out);
            self.split(bx, by, cx, cy, mx, my, max_error, out);
        } else {
            out.push([self.point(ax, ay), self.point(bx, by), self.point(cx, cy)]);
        }
    }

    fn point(&self, i: usize, j: usize) -> P3 {
        [self.x0 + i as f32 * self.cell, self.heights[j * SIZE + i], self.z0 + j as f32 * self.cell]
    }

    /// Lowest and highest sample.
    pub fn range(&self) -> (f32, f32) {
        self.heights.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &y| (lo.min(y), hi.max(y)))
    }
}

/// The winding of the ground's triangles as seen from above must match the roofs': counter-clockwise from +y.
pub fn face_up(t: &mut [P3; 3]) {
    let (a, b, c) = (t[0], t[1], t[2]);
    let ny = (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]);
    if ny < 0.0 {
        t.swap(1, 2);
    }
}
