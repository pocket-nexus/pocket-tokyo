//! What a frame draws. The pack cuts the city into regions, blocks and cells
//! (`tokyo_pack`); for an eye and its frustum, `select` lists the draws: the
//! far level of the regions that lie far away, the mid level of the cells in
//! view of nearer blocks, the near level of the cells near the eye, and of
//! every batch ordered by sector only the arc that can face the eye.
//! A device binds its programs and textures and issues the draws.

use crate::mat;
use crate::math::*;
use alloc::vec::Vec;
use tokyo_pack::{flag, kind, Batch, Block, Cell, City, Draws, Region, LODS, SECTORS};

/// The pack's tables.
pub struct Tables<'a> {
    pub city: &'a City,
    pub regions: &'a [Region],
    pub blocks: &'a [Block],
    pub cells: &'a [Cell],
    pub batches: &'a [Batch],
    pub spans: &'a [u32],
    /// The batches of kind `CARD`: drawn whenever they are in view.
    pub cards: &'a [u32],
}

/// A draw of this frame: indices `from..to` of a batch, in the frame of a block or of a region.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub batch: u32,
    pub place: u32,
    /// The cell whose near level this is; `u32::MAX` at the other levels.
    pub cell: u32,
    pub from: u32,
    pub to: u32,
    pub region: bool,
    /// The sector its faces look to, `SECTORS` for those that look up or down; `MIXED` for a draw of several.
    pub sector: u8,
}

pub const MIXED: u8 = 255;

/// The distances at which the levels of detail hand over.
#[derive(Clone, Copy, Debug)]
pub struct Reach {
    pub near: f32,
    pub mid: f32,
    /// Leave out the walls that face away from the eye.
    pub sectors: bool,
    /// One draw per sector, for a machine that lights each with one colour.
    pub split: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Counts {
    /// Places drawn at each level of detail: cells near, blocks mid, regions far.
    pub places: [u32; LODS],
    /// Triangles left out because they face away.
    pub turned: u32,
    /// The nearest of what is drawn at the mid level, metres from the eye.
    pub mid_from: f32,
}

/// The sectors of the compass whose walls can face `eye` from somewhere in a box: the first, and how many.
/// `None`: all of them.
pub fn facing(eye: V3, min: &[f32; 3], max: &[f32; 3]) -> Option<(usize, usize)> {
    const PAD: f32 = 4.0;
    if eye.x > min[0] - PAD && eye.x < max[0] + PAD && eye.z > min[2] - PAD && eye.z < max[2] + PAD {
        return None;
    }
    // Directions from the box's corners to the eye, as angles about the direction from its middle.
    let (cx, cz) = ((min[0] + max[0]) * 0.5, (min[2] + max[2]) * 0.5);
    let mid = atan2(eye.z - cz, eye.x - cx);
    let (mut lo, mut hi) = (0.0f32, 0.0f32);
    for (x, z) in [(min[0], min[2]), (max[0], min[2]), (min[0], max[2]), (max[0], max[2])] {
        let d = wrap_angle(atan2(eye.z - z, eye.x - x) - mid);
        lo = lo.min(d);
        hi = hi.max(d);
    }
    // A wall faces a direction when its normal is within a quarter turn of it.
    let step = TAU / SECTORS as f32;
    let first = floor((mid + lo - PI * 0.5 + PI) / step) as i32;
    let last = floor((mid + hi + PI * 0.5 + PI) / step) as i32;
    let count = (last - first + 1) as usize;
    if count >= SECTORS {
        return None;
    }
    Some((first.rem_euclid(SECTORS as i32) as usize, count))
}

/// The direction the middle of a sector looks to.
pub fn sector_normal(k: usize) -> V3 {
    let a = -PI + (k as f32 + 0.5) * TAU / SECTORS as f32;
    v3(cos(a), 0.0, sin(a))
}

/// Where a place's vertices start and how far they reach: `top` for the ground and roofs, which keep to their
/// place; walls and solids reach its margin.
pub fn frame_of(c: &City, place: u32, region: bool, top: bool) -> ([f32; 3], [f32; 3]) {
    let per = c.region_blocks as usize;
    let (nx, side) = if region { (c.blocks_x as usize / per, c.block * per as f32) } else { (c.blocks_x as usize, c.block) };
    let m = if top { 0.0 } else { c.margin };
    ([c.x0 + (place as usize % nx) as f32 * side - m, c.y0, c.z0 + (place as usize / nx) as f32 * side - m], [side + 2.0 * m, c.y_span, side + 2.0 * m])
}

/// The draws of one record ordered by sector (`s`: where each sector starts, then the faces that look up or
/// down, then the end), of which `arc` can face the eye.
fn arcs(list: &mut Vec<Item>, turned: &mut u32, item: Item, s: &[u32], arc: Option<(usize, usize)>, split: bool) {
    let groups = SECTORS + 1;
    let mut push = |from: u32, to: u32, sector: u8| {
        if to <= from {
            return;
        }
        // (a range that continues the one before it is the same draw)
        match list.last_mut() {
            Some(last) if last.batch == item.batch && last.to == from && last.sector == sector => last.to = to,
            _ => list.push(Item { from, to, sector, ..item }),
        }
    };
    let (first, count) = arc.unwrap_or((0, SECTORS));
    if split {
        for k in first..first + count {
            let k = k % SECTORS;
            push(s[k], s[k + 1], k as u8);
        }
        push(s[SECTORS], s[groups], SECTORS as u8);
        if arc.is_some() {
            let drawn: u32 = (first..first + count).map(|k| s[k % SECTORS + 1] - s[k % SECTORS]).sum();
            *turned += (s[SECTORS] - s[0] - drawn) / 3;
        }
        return;
    }
    if arc.is_none() {
        push(s[0], s[groups], MIXED);
        return;
    }
    let end = first + count;
    let ranges = if end <= SECTORS { [(s[first], s[end]), (s[SECTORS], s[groups])] } else { [(s[0], s[end - SECTORS]), (s[first], s[groups])] };
    *turned += ((s[groups] - s[0]) - (ranges[0].1 - ranges[0].0) - (ranges[1].1 - ranges[1].0)) / 3;
    push(ranges[0].0, ranges[0].1, MIXED);
    push(ranges[1].0, ranges[1].1, MIXED);
}

/// Fills `lists` (one per kind of batch) with the draws of a frame seen from `eye` through `planes`.
/// `ready(cell)`: whether a cell's near level is in memory; a cell whose is not is drawn at the mid level.
pub fn select(t: &Tables, planes: &[[f32; 4]; 6], eye: V3, show: &Reach, ready: &dyn Fn(usize) -> bool, lists: &mut [Vec<Item>]) -> Counts {
    let mut stats = Counts { places: [0; LODS], turned: 0, mid_from: f32::MAX };
    for l in lists.iter_mut() {
        l.clear();
    }
    let this = t;
    let c = this.city;
    let n = c.cells as usize;
    let per_place = n * n;
    let per = c.region_blocks as usize;
    let (bnx, rnx) = (c.blocks_x as usize, c.blocks_x as usize / per);
    let region_side = c.block * per as f32;
    let faced = c.flags & flag::FACED != 0;
    let by_sector = |b: &Batch| b.kind == kind::WALL || (faced && b.kind == kind::SOLID);
    // A cell's near batches, when their boxes are in view.
    let near = |lists: &mut [Vec<Item>], stats: &mut Counts, draws: Draws, place: u32, cell: u32, bounds: &Cell| {
        let arc = if show.sectors { facing(eye, &bounds.min, &bounds.max) } else { None };
        for bi in draws.first..draws.first + draws.count {
            let b = &this.batches[bi as usize];
            if !mat::visible(planes, &b.min, &b.max) {
                continue;
            }
            let item = Item { batch: bi, place, cell, from: 0, to: b.idx_count, region: false, sector: if b.kind == kind::TOP { SECTORS as u8 } else { MIXED } };
            if b.spans == u32::MAX || !by_sector(b) {
                lists[b.kind as usize].push(item);
                continue;
            }
            let s = &this.spans[b.spans as usize..b.spans as usize + SECTORS + 2];
            arcs(&mut lists[b.kind as usize], &mut stats.turned, item, s, arc, show.split);
        }
    };
    // Batches ordered by cell: the cells that are `on`, and of the walls in each only the arc of the compass
    // that can face the eye from the cell's box.
    let by_cell = |lists: &mut [Vec<Item>], stats: &mut Counts, draws: Draws, place: u32, region: bool, on: &[bool; 64], boxes: &[([f32; 3], [f32; 3]); 64]| {
        // The arc of each cell, once for all of the place's batches.
        let mut arc_of = [None; 64];
        if show.sectors {
            for ci in 0..per_place {
                if on[ci] {
                    arc_of[ci] = facing(eye, &boxes[ci].0, &boxes[ci].1);
                }
            }
        }
        for bi in draws.first..draws.first + draws.count {
            let b = &this.batches[bi as usize];
            let list = &mut lists[b.kind as usize];
            let item = Item { batch: bi, place, cell: u32::MAX, from: 0, to: 0, region, sector: if b.kind == kind::TOP { SECTORS as u8 } else { MIXED } };
            if !by_sector(b) {
                let s = &this.spans[b.spans as usize..b.spans as usize + per_place + 1];
                for ci in 0..per_place {
                    if on[ci] && s[ci + 1] > s[ci] {
                        match list.last_mut() {
                            Some(last) if last.batch == bi && last.to == s[ci] => last.to = s[ci + 1],
                            _ => list.push(Item { from: s[ci], to: s[ci + 1], ..item }),
                        }
                    }
                }
                continue;
            }
            let groups = SECTORS + 1;
            let s = &this.spans[b.spans as usize..b.spans as usize + per_place * groups + 1];
            for ci in 0..per_place {
                if !on[ci] {
                    continue;
                }
                arcs(list, &mut stats.turned, item, &s[ci * groups..ci * groups + groups + 1], arc_of[ci], show.split);
            }
        }
    };
    let half = n / per;
    for (ri, region) in this.regions.iter().enumerate() {
        let (rx, rz) = (ri % rnx, ri / rnx);
        let (x0, z0) = (c.x0 + rx as f32 * region_side, c.z0 + rz as f32 * region_side);
        // (walls and solids may stand out of their place by the margin; batches carry their own boxes)
        if !mat::visible(planes, &[x0 - c.margin, region.y_min, z0 - c.margin], &[x0 + region_side + c.margin, region.y_max, z0 + region_side + c.margin]) {
            continue;
        }
        // The region's cells that its far level draws: those of its blocks that are far away.
        let mut far = [false; 64];
        let mut far_boxes = [([0.0f32; 3], [0.0f32; 3]); 64];
        let mut any_far = false;
        let far_side = region_side / n as f32;
        for k in 0..per * per {
            let (kx, kz) = (k % per, k / per);
            let (bx, bz) = (rx * per + kx, rz * per + kz);
            let bi = bz * bnx + bx;
            let block = &this.blocks[bi];
            let (bx0, bz0) = (c.x0 + bx as f32 * c.block, c.z0 + bz as f32 * c.block);
            if !mat::visible(planes, &[bx0 - c.margin, region.y_min, bz0 - c.margin], &[bx0 + c.block + c.margin, region.y_max, bz0 + c.block + c.margin]) {
                continue;
            }
            if block.mid.count == 0 || mat::box_distance(eye, &[bx0, block.y_min, bz0], &[bx0 + c.block, block.y_max, bz0 + c.block]) >= show.mid {
                for j in 0..half {
                    for i in 0..half {
                        let ci = (kz * half + j) * n + kx * half + i;
                        let (cx0, cz0) = (x0 + (kx * half + i) as f32 * far_side, z0 + (kz * half + j) as f32 * far_side);
                        let cell = ([cx0, region.y_min, cz0], [cx0 + far_side, region.y_max, cz0 + far_side]);
                        // (a cell's walls may stand a little out of it)
                        if mat::visible(planes, &[cell.0[0] - 30.0, cell.0[1], cell.0[2] - 30.0], &[cell.1[0] + 30.0, cell.1[1], cell.1[2] + 30.0]) {
                            far[ci] = true;
                            far_boxes[ci] = cell;
                            any_far = true;
                        }
                    }
                }
                continue;
            }
            // A block within the middle distance: each of its cells in view at the near or the mid level.
            stats.places[1] += 1;
            let cells = &this.cells[bi * per_place..(bi + 1) * per_place];
            let mut mid = [false; 64];
            let mut boxes = [([0.0f32; 3], [0.0f32; 3]); 64];
            for (ci, cell) in cells.iter().enumerate() {
                if cell.min[1] > cell.max[1] || !mat::visible(planes, &cell.min, &cell.max) {
                    continue;
                }
                let id = bi * per_place + ci;
                let d = mat::box_distance(eye, &cell.min, &cell.max);
                if d < show.near && ready(id) {
                    stats.places[0] += 1;
                    near(lists, &mut stats, cell.near, bi as u32, id as u32, cell);
                } else {
                    stats.mid_from = min(stats.mid_from, d);
                    mid[ci] = true;
                    boxes[ci] = (cell.min, cell.max);
                }
            }
            by_cell(lists, &mut stats, block.mid, bi as u32, false, &mid, &boxes);
        }
        if any_far {
            stats.places[2] += 1;
            by_cell(lists, &mut stats, region.far, ri as u32, true, &far, &far_boxes);
        }
    }
    for &bi in this.cards {
        let b = &this.batches[bi as usize];
        if mat::visible(planes, &b.min, &b.max) {
            lists[b.kind as usize].push(Item { batch: bi, place: b.spans, cell: u32::MAX, from: 0, to: b.idx_count, region: false, sector: MIXED });
        }
    }
    stats
}

/// Keeps a frame's triangles near a budget by the distances at which the levels of detail hand over: in
/// quickly when a frame draws too many, out slowly when there is room.
#[derive(Clone, Copy, Debug)]
pub struct Governor {
    pub budget: u32,
    pub on: bool,
    /// The near and the mid distance when the budget allows them in full.
    pub reach: (f32, f32),
    pub scale: f32,
}

impl Governor {
    pub fn new(budget: u32, reach: (f32, f32)) -> Governor {
        Governor { budget, on: true, reach, scale: 1.0 }
    }

    /// After a frame that drew `drawn` triangles: the distances for the next.
    pub fn after(&mut self, drawn: u32, show: &mut Reach) {
        if !self.on {
            return;
        }
        if drawn > self.budget {
            self.scale *= 0.97;
        } else if (drawn as f32) < self.budget as f32 * 0.88 {
            self.scale *= 1.008;
        }
        self.scale = clamp(self.scale, 0.25, 1.0);
        show.near = self.reach.0 * self.scale;
        show.mid = self.reach.1 * self.scale;
    }
}
