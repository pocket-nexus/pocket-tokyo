//! The buildings of a tile, from the triangles the reference's mesher made:
//! every face that looks up becomes a roof under the tile's picture from
//! above, every other face a wall under the facade pictures, and the lattice
//! tower painted steel.

use crate::city::{sector_of, SolidTri, TopTri, WallTri};
use crate::geom::{bands, clip, cross, dot, face, norm, srgb8, P3, V};
use crate::ir::Bundle;
use serde_json::Value;

/// Surface kinds of the mesher (`web/src/world/constants.js`).
const WALL: f32 = 0.0;
const LATTICE: f32 = 4.0;
/// Faces whose normal rises more than this are drawn from above.
pub const TOP_NY: f32 = 0.3;
/// Window bay width (m) per facade category, as the mesher fits them (`BAY` in `web/src/world/meshing.js`).
pub const BAY: [f32; 6] = [3.4, 3.3, 3.2, 3.0, 3.4, 1.5];

/// Where the facade pictures keep each kind of wall (`web/src/pocket/facades.js`).
pub struct Facade {
    width: f32,
    column: f32,
    pub bays: f32,
    pub floors: f32,
    margin: f32,
    floor_height: f32,
    house: usize,
    apartment: usize,
    mixed: usize,
    commercial: usize,
    office: usize,
    glass: usize,
    tower: usize,
    plain: usize,
    plain_bay: f32,
}

impl Facade {
    pub fn from_json(v: &Value) -> Result<Facade, String> {
        let f = |k: &str| v[k].as_f64().map(|x| x as f32).ok_or_else(|| format!("facade.json: no {k}"));
        let column = |name: &str| v["columns"].as_array().and_then(|a| a.iter().position(|c| c["name"] == name)).ok_or_else(|| format!("facade.json: no column {name}"));
        let plain = column("plain")?;
        Ok(Facade {
            width: f("width")?,
            column: f("column")?,
            bays: f("bays")?,
            floors: f("floors")?,
            margin: f("margin")?,
            floor_height: f("floorHeight")?,
            house: column("house")?,
            apartment: column("apartment")?,
            mixed: column("mixed")?,
            commercial: column("commercial")?,
            office: column("office")?,
            glass: column("glass")?,
            tower: column("tower")?,
            plain,
            plain_bay: v["columns"][plain]["bay"].as_f64().unwrap_or(3.0) as f32,
        })
    }

    /// `u` of the picture at `bays` (0 ..= self.bays) into a column.
    fn u(&self, column: usize, bays: f32) -> f32 {
        (column as f32 * self.column + (self.margin + bays) * self.column / (self.bays + 2.0 * self.margin)) / self.width
    }

    /// The column whose storeys repeat upwards, and the one that starts at the ground with a row of shops (if the
    /// category has shops).
    fn columns_of(&self, cat: i32, wall_height: f32) -> (usize, Option<usize>) {
        match cat {
            0 => (self.house, None),
            1 => (self.apartment, None),
            2 => (self.apartment, Some(self.mixed)),
            3 => (self.office, Some(self.commercial)),
            4 => (self.office, None),
            _ => (if wall_height > 90.0 { self.tower } else { self.glass }, None),
        }
    }
}

pub fn hash(a: u32, b: u32) -> u32 {
    let mut h = a.wrapping_mul(0x27d4_eb2d) ^ b.wrapping_mul(0x1656_67b1);
    h = (h ^ (h >> 15)).wrapping_mul(0x85eb_ca6b);
    h = (h ^ (h >> 13)).wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

/// What every wall of one building shares.
#[derive(Clone, Copy, Debug)]
pub struct Building {
    pub cat: i32,
    pub color: [u8; 3],
    /// Storey height, metres.
    pub floor: f32,
    /// The height the storeys count from.
    pub base: f32,
    pub wall_height: f32,
    /// Brightness of the lit rooms (half scale) and how late in the dusk they come on.
    pub gain: u8,
    pub late: u8,
    /// A number of its own, for the choices that differ from building to building.
    pub id: u32,
    /// Whether the building is the lattice tower, which no simpler level of detail stands in for.
    pub lattice: bool,
}

impl Building {
    pub const PLAIN: Building = Building { cat: 3, color: [128, 128, 126], floor: 3.2, base: 0.0, wall_height: 10.0, gain: 0, late: 0, id: 0, lattice: false };
}

pub struct Built {
    /// Roofs, each with the index of its building in `buildings` (`u32::MAX`: a roof under its aerial photo,
    /// whose building is found by where it lies).
    pub tops: Vec<(TopTri, u32)>,
    pub walls: Vec<WallTri>,
    pub solids: Vec<SolidTri>,
    pub buildings: Vec<Building>,
}

/// Position, then `u` (bays) and `v` (metres above the base).
pub type WV = V<5>;

/// Walls of one building under the facade pictures.
pub struct Walls<'a> {
    pub f: &'a Facade,
    pub out: &'a mut Vec<WallTri>,
}

impl Walls<'_> {
    /// A piece whose `u` is in bays of `column` and whose `v` is already in repeats of the picture.
    fn emit(&mut self, b: &Building, n: P3, column: usize, tri: &[WV; 3], shift: f32) {
        let f = self.f;
        let moved = tri.map(|v| [v[0], v[1], v[2], v[3] + shift, v[4]]);
        let sector = sector_of(n);
        bands(&moved, 3, f.bays, |k, piece| {
            let (_, area) = face([piece[0][0], piece[0][1], piece[0][2]], [piece[1][0], piece[1][1], piece[1][2]], [piece[2][0], piece[2][1], piece[2][2]]);
            if area < 1e-5 {
                return;
            }
            self.out.push(WallTri {
                p: piece.map(|v| [v[0], v[1], v[2]]),
                n,
                uv: piece.map(|v| [f.u(column, (v[3] - k as f32 * f.bays).clamp(0.0, f.bays)), v[4]]),
                color: b.color,
                gain: b.gain,
                late: b.late,
                sector,
            });
        });
    }

    /// A face without windows: bays and storeys measured along the face itself.
    pub fn plain(&mut self, b: &Building, n: P3, p: &[P3; 3]) {
        let f = self.f;
        let t = if n[1].abs() > 0.9 { [1.0, 0.0, 0.0] } else { norm(cross([0.0, 1.0, 0.0], n)) };
        let up = cross(n, t);
        let mut tri = p.map(|q| [q[0], q[1], q[2], dot(q, t) / f.plain_bay, -dot(q, up) / f.floor_height / f.floors]);
        // A face narrower than the picture starts where it fits whole: only wider ones are cut.
        let lo = tri.iter().map(|v| v[3]).fold(f32::INFINITY, f32::min);
        let span = tri.iter().map(|v| v[3]).fold(f32::NEG_INFINITY, f32::max) - lo;
        if span < f.bays {
            let room = f.bays - span;
            let start = (hash(b.id, (p[0][0] * 7.0 + p[0][2] * 13.0 + p[0][1] * 3.0) as i32 as u32) % 64) as f32 / 64.0 * room;
            for v in &mut tri {
                v[3] += start - lo;
            }
        }
        self.emit(b, n, f.plain, &tri, 0.0);
    }

    /// One wall with windows: triangles in one plane, `bays` window bays across. `salt` tells this wall from the
    /// building's others.
    pub fn windows(&mut self, b: &Building, n: P3, tris: &[[WV; 3]], bays: f32, salt: u32) {
        let f = self.f;
        // A wall narrower than the picture's bays starts at any of them; a wider one is cut where the picture ends.
        let room = (f.bays - bays).max(0.0) as u32;
        let shift = if room > 0 { (hash(b.id, salt) % (room + 1)) as f32 } else { 0.0 };
        let (repeating, grounded) = f.columns_of(b.cat, b.wall_height);
        let lift = (hash(b.id, 3) % f.floors as u32) as f32;
        let floor = b.floor.max(1.0);
        // Storeys the grounded column covers; above them the building continues in the column that repeats.
        let ground_top = if grounded.is_some() { f.floors * floor } else { f32::NEG_INFINITY };
        for tri in tris {
            let upper = clip(tri, |v| v[4] - ground_top);
            for m in 1..upper.len().saturating_sub(1) {
                let piece = [upper[0], upper[m], upper[m + 1]].map(|v| [v[0], v[1], v[2], v[3], -(v[4] / floor + lift) / f.floors]);
                self.emit(b, n, repeating, &piece, shift);
            }
            if let Some(column) = grounded {
                let lower = clip(tri, |v| ground_top - v[4]);
                for m in 1..lower.len().saturating_sub(1) {
                    let piece = [lower[0], lower[m], lower[m + 1]].map(|v| [v[0], v[1], v[2], v[3], 1.0 - v[4] / floor / f.floors]);
                    self.emit(b, n, column, &piece, shift);
                }
            }
        }
    }
}

/// Splits the triangles of a tile's buildings by the program that will draw them.
pub fn build(b: &Bundle, f: &Facade, tile: (i32, i32)) -> Built {
    let pos = b.f32("bldg.position");
    let col = b.f32("bldg.color");
    let fac = b.f32("bldg.facade");
    let kind = b.f32("bldg.kind");
    let ends = b.f32("bldg.ends");
    let info = b.f32("foot.info");
    let mut out = Built { tops: Vec::new(), walls: Vec::new(), solids: Vec::new(), buildings: Vec::with_capacity(ends.len()) };
    let p3 = |i: usize| [pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]];
    let frac = |x: f32| x - x.floor();
    let n = pos.len() / 9;

    // ---- what each building's walls share, from its first wall with windows (or its first vertex)
    let mut first = 0usize;
    for (bi, &end) in ends.iter().enumerate() {
        let end = end as usize;
        let mut pick = None;
        let mut lattice = false;
        for v in (first..end).step_by(3) {
            lattice |= kind[v * 4 + 2] == LATTICE;
            if pick.is_none() && kind[v * 4 + 2] == WALL && kind[v * 4 + 3] > 0.5 {
                pick = Some(v);
            }
        }
        let base = info.get(bi * 7).copied().unwrap_or(0.0);
        let id = hash(bi as u32 + 1, (tile.0 as u32).wrapping_mul(73) ^ (tile.1 as u32).wrapping_mul(131));
        out.buildings.push(match pick.or((first < end).then_some(first)) {
            Some(v) => {
                let seed = fac[v * 4 + 3];
                Building {
                    cat: (kind[v * 4 + 1] as i32) % 8,
                    color: [srgb8(col[v * 3]), srgb8(col[v * 3 + 1]), srgb8(col[v * 3 + 2])],
                    floor: fac[v * 4 + 2].max(1.0),
                    base,
                    wall_height: info.get(bi * 7 + 1).copied().unwrap_or(kind[v * 4]),
                    // (the material's own spread of brightness from building to building)
                    gain: ((0.35 + 1.55 * frac(seed * 11.7)) / 2.0 * 255.0) as u8,
                    late: (hash(id, 7) & 255) as u8,
                    id,
                    lattice,
                }
            }
            None => Building { base, id, ..Building::PLAIN },
        });
        first = end;
    }

    // ---- its triangles
    let mut building = 0usize;
    let mut run: Vec<usize> = Vec::new();
    let mut tris: Vec<[WV; 3]> = Vec::new();
    let mut t = 0;
    while t < n {
        let i = t * 3;
        while building < ends.len() && (i as f32) >= ends[building] {
            building += 1;
        }
        let (a, bb, c) = (p3(i), p3(i + 1), p3(i + 2));
        let (nrm, area) = face(a, bb, c);
        let k = kind[i * 4 + 2];
        if area < 1e-5 {
            t += 1;
            continue;
        }
        if k == LATTICE {
            // Colours brighter than 1 are lamps (`web/src/world/tower.js`).
            let rgb = [col[i * 3], col[i * 3 + 1], col[i * 3 + 2]];
            let top = rgb[0].max(rgb[1]).max(rgb[2]);
            // (alpha: how far a face shines by its own colour at night: lamps fully, the floodlit steel by half)
            let color = if top > 1.5 { [srgb8(rgb[0] / top), srgb8(rgb[1] / top), srgb8(rgb[2] / top), 255] } else { [srgb8(rgb[0]), srgb8(rgb[1]), srgb8(rgb[2]), 120] };
            out.solids.push(SolidTri { p: [a, bb, c], n: [nrm; 3], color });
            t += 1;
            continue;
        }
        if k != WALL && nrm[1] >= TOP_NY {
            out.tops.push((TopTri::flat([a, bb, c]), building as u32));
            t += 1;
            continue;
        }
        // A wall of the mesher is a run of triangles with one normal and one bay width: the bays it spans decide
        // where in the picture's eight it starts.
        run.clear();
        let bay = kind[i * 4 + 3];
        let mut e = t;
        while e < n {
            let j = e * 3;
            if (j as f32) >= ends.get(building).copied().unwrap_or(f32::INFINITY) {
                break;
            }
            let (q, _) = face(p3(j), p3(j + 1), p3(j + 2));
            let kj = kind[j * 4 + 2];
            let same = kind[j * 4 + 3] == bay && (kj == WALL) == (k == WALL) && dot(q, nrm) > 0.999 && !(kj != WALL && q[1] >= TOP_NY) && kj != LATTICE;
            if !same {
                break;
            }
            run.push(e);
            e += 1;
        }
        if run.is_empty() {
            run.push(t);
            e = t + 1;
        }
        let info = out.buildings.get(building).copied().unwrap_or(Building::PLAIN);
        let mut w = Walls { f, out: &mut out.walls };
        if k == WALL && bay > 0.5 {
            let bays = run.iter().flat_map(|&e| (0..3).map(move |v| e * 3 + v)).map(|v| fac[v * 4]).fold(0.0f32, f32::max).round();
            tris.clear();
            for &e in &run {
                let j = e * 3;
                tris.push([0, 1, 2].map(|v| {
                    let q = p3(j + v);
                    [q[0], q[1], q[2], fac[(j + v) * 4], fac[(j + v) * 4 + 1]]
                }));
            }
            w.windows(&info, nrm, &tris, bays, run[0] as u32);
        } else {
            for &e in &run {
                let j = e * 3;
                let own = Building { color: [srgb8(col[j * 3]), srgb8(col[j * 3 + 1]), srgb8(col[j * 3 + 2])], ..info };
                w.plain(&own, nrm, &[p3(j), p3(j + 1), p3(j + 2)]);
            }
        }
        t = e;
    }
    // Roofs under their aerial photo: the photo is in the picture from above.
    let roof = b.f32("roof.position");
    for tri in roof.chunks_exact(9) {
        let p = [[tri[0], tri[1], tri[2]], [tri[3], tri[4], tri[5]], [tri[6], tri[7], tri[8]]];
        let (nrm, area) = face(p[0], p[1], p[2]);
        if area < 1e-5 {
            continue;
        }
        if nrm[1] >= 0.05 {
            out.tops.push((TopTri::flat(p), u32::MAX));
        } else {
            Walls { f, out: &mut out.walls }.plain(&Building::PLAIN, nrm, &p);
        }
    }
    out
}
