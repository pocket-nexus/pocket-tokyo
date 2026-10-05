//! City compiler of Pocket Tokyo.
//!
//!   tokyo-cook --in <CityIR directory> --out <directory> --profile profiles/vita60.json
//!
//! Reads what the export page wrote (`web/src/pocket/export.js`) and writes
//! `city.pack` and `receipt.json`: the city cut into regions, blocks and cells
//! at three levels of detail, each a few draws, the pictures from above and
//! the facade pictures block-compressed, and the city's heights for the
//! shadows.

mod buildings;
mod city;
mod etc1;
mod geom;
mod handheld;
mod heights;
mod ir;
mod landmark;
mod prisms;
mod simplify;
mod target;
mod terrain;
mod tex;

use buildings::{Building, Facade};
use city::{Level, SolidTri, TopTri, WallTri};
use geom::{clip, face, snorm8, srgb8, Mesh, P3, V};
use rayon::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::time::Instant;
use target::{psp565, store, Raw, Target};
use tokyo_pack::{self as pack, flag, kind, Batch, Block, Cell, City, Draws, GroundLevel, Landmark, NearCell, Region, SolidVertex, TopVertex, WallVertex, LODS, SECTORS};

/// Heights are stored over this range (metres above Tokyo Peil).
const Y0: f32 = -16.0;
const Y_SPAN: f32 = 512.0;
/// Walls and solids may reach this far out of their block.
const MARGIN: f32 = 128.0;
/// Tiles of the export along a block's side.
const BLOCK_TILES: i32 = 2;

fn main() {
    if let Err(e) = run() {
        eprintln!("tokyo-cook: {e}");
        std::process::exit(1);
    }
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| format!("{}: {e}", path.display()))
}

/// The tiles a triangle's plan touches, shrunk by a hair so that one lying along a border belongs to one side.
fn tile_range(p: &[P3; 3], tile: f32) -> (i32, i32, i32, i32) {
    let e = 1e-3;
    let lo = |k: usize| p[0][k].min(p[1][k]).min(p[2][k]) + e;
    let hi = |k: usize| p[0][k].max(p[1][k]).max(p[2][k]) - e;
    let t = |v: f32| (v / tile).floor() as i32;
    (t(lo(0)), t(lo(2)), t(hi(0)).max(t(lo(0))), t(hi(2)).max(t(lo(2))))
}

/// Cuts a triangle with `N` numbers per vertex (position first) by the tile grid.
fn by_tile<const N: usize>(tri: &[V<N>; 3], tile: f32, mut put: impl FnMut((i32, i32), [V<N>; 3])) {
    let p = tri.map(|v| [v[0], v[1], v[2]]);
    let (x0, z0, x1, z1) = tile_range(&p, tile);
    if x0 == x1 && z0 == z1 {
        put((x0, z0), *tri);
        return;
    }
    for tz in z0..=z1 {
        for tx in x0..=x1 {
            let (ax, az) = (tx as f32 * tile, tz as f32 * tile);
            let mut poly = clip(tri, |v| v[0] - ax);
            poly = clip(&poly, |v| ax + tile - v[0]);
            poly = clip(&poly, |v| v[2] - az);
            poly = clip(&poly, |v| az + tile - v[2]);
            for i in 1..poly.len().saturating_sub(1) {
                let t = [poly[0], poly[i], poly[i + 1]];
                let (_, area) = face([t[0][0], t[0][1], t[0][2]], [t[1][0], t[1][1], t[1][2]], [t[2][0], t[2][1], t[2][2]]);
                if area > 1e-5 {
                    put((tx, tz), t);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    min: P3,
    max: P3,
}

impl Bounds {
    fn new() -> Bounds {
        Bounds { min: [f32::INFINITY; 3], max: [f32::NEG_INFINITY; 3] }
    }
    fn add(&mut self, p: &[P3]) {
        for q in p {
            for k in 0..3 {
                self.min[k] = self.min[k].min(q[k]);
                self.max[k] = self.max[k].max(q[k]);
            }
        }
    }
}

fn qy(y: f32) -> u16 {
    ((y - Y0) / Y_SPAN * 65535.0).round().clamp(0.0, 65535.0) as u16
}
fn q_in(v: f32, origin: f32, side: f32) -> u16 {
    ((v - origin) / side * 65535.0).round().clamp(0.0, 65535.0) as u16
}
fn q_wide(v: f32, origin: f32, side: f32) -> u16 {
    ((v - origin + MARGIN) / (side + 2.0 * MARGIN) * 65535.0).round().clamp(0.0, 65535.0) as u16
}
fn n8(n: P3) -> [i8; 3] {
    [snorm8(n[0]), snorm8(n[1]), snorm8(n[2])]
}

/// Triangles welded into one draw's worth of vertices: at most 65 535.
struct Chunk<T> {
    vertices: Vec<T>,
    tris: Vec<[u16; 3]>,
    /// For each triangle, which of the input it was.
    source: Vec<usize>,
    bounds: Bounds,
}

fn chunks<T: Copy + Eq + Hash, S>(tris: &[S], vertex: impl Fn(&S) -> ([T; 3], [P3; 3])) -> Vec<Chunk<T>> {
    let empty = || Chunk { vertices: Vec::new(), tris: Vec::new(), source: Vec::new(), bounds: Bounds::new() };
    let mut out = Vec::new();
    let mut mesh = Mesh::<T>::default();
    let mut chunk = empty();
    for (i, s) in tris.iter().enumerate() {
        if mesh.full() {
            chunk.vertices = std::mem::take(&mut mesh.vertices);
            out.push(std::mem::replace(&mut chunk, empty()));
            mesh = Mesh::default();
        }
        let (v, p) = vertex(s);
        if let Some(t) = mesh.tri(v) {
            chunk.tris.push(t);
            chunk.source.push(i);
            chunk.bounds.add(&p);
        }
    }
    if !chunk.tris.is_empty() {
        chunk.vertices = mesh.vertices;
        out.push(chunk);
    }
    out
}

/// Everything the draws of the pack are made of.
#[derive(Default)]
struct Out {
    batches: Vec<Batch>,
    spans: Vec<u32>,
    vtop: Vec<u8>,
    vwal: Vec<u8>,
    vsol: Vec<u8>,
    idx: Vec<u16>,
    /// Triangles by level of detail and program.
    stats: [[usize; 3]; LODS],
}

impl Out {
    /// A batch over vertices already stored at `vtx_first`: `tris` in the order given, and, when they come in
    /// groups (`group[k]` for triangle `k`, never falling, of `groups`), where each group starts.
    fn batch(&mut self, kind: u32, vtx_first: usize, vtx_count: usize, bounds: &Bounds, tris: &[[u16; 3]], groups: Option<(&[usize], usize)>) {
        let mut spans = u32::MAX;
        if let Some((group, n)) = groups {
            spans = self.spans.len() as u32;
            let mut at = 0;
            for g in 0..=n {
                while at < group.len() && group[at] < g {
                    at += 1;
                }
                self.spans.push(at as u32 * 3);
            }
        }
        self.batches.push(Batch { kind, vtx_first: vtx_first as u32, vtx_count: vtx_count as u32, idx_first: self.idx.len() as u32, idx_count: tris.len() as u32 * 3, min: bounds.min, max: bounds.max, spans });
        self.idx.extend(tris.iter().flatten());
    }
}

/// How a level's triangles become vertices of a place whose corner is `(x0, z0)` and whose side is `side`.
#[derive(Clone, Copy)]
struct Frame<'a> {
    x0: f32,
    z0: f32,
    side: f32,
    /// The city as heights, for how much of the sky each vertex sees.
    sky: &'a heights::Heights,
    target: Target,
}

/// A signed 16-bit position over a span: the handhelds' vertex formats have no unsigned one.
fn s16(q: u16) -> [u8; 2] {
    ((q as i32 - 32768) as i16).to_le_bytes()
}

impl Frame<'_> {
    fn top(&self, t: &TopTri) -> ([Raw; 3], [P3; 3]) {
        let v = [0, 1, 2].map(|k| {
            let pos = [q_in(t.p[k][0], self.x0, self.side), qy(t.p[k][1]), q_in(t.p[k][2], self.z0, self.side)];
            let ao = self.sky.open(t.p[k], t.n[k]);
            match self.target {
                Target::Vita | Target::Gles3 => Raw::of(&TopVertex { pos, ao, pad: 0, normal: n8(t.n[k]), pad2: 0 }),
                // The GE: colour, position. Its texture matrix makes the coordinates in the picture from x and z,
                // and every top is lit as one group.
                Target::Psp => Raw::from(&[&psp565([ao; 3]), &s16(pos[0]), &s16(pos[1]), &s16(pos[2])]),
                // The PICA200: position, then what the point sees of the sky. Its program lights every top alike.
                Target::Pica | Target::Gles => Raw::from(&[&s16(pos[0]), &s16(pos[1]), &s16(pos[2]), &[ao, 0]]),
            }
        });
        let _ = n8;
        (v, t.p)
    }
    fn wall(&self, w: &WallTri) -> ([Raw; 3], [P3; 3]) {
        let v = [0, 1, 2].map(|k| {
            let pos = [q_wide(w.p[k][0], self.x0, self.side), qy(w.p[k][1]), q_wide(w.p[k][2], self.z0, self.side)];
            let (ao, n) = (self.sky.open(w.p[k], w.n), n8(w.n));
            let uv = [(w.uv[k][0] * 32767.0).round() as i16, (w.uv[k][1] / pack::FACADE_V * 32767.0).round().clamp(-32767.0, 32767.0) as i16];
            match self.target {
                Target::Vita | Target::Gles3 => Raw::of(&WallVertex { pos, ao, gain: w.gain, normal: n, late: w.late, color: [w.color[0], w.color[1], w.color[2], 0], uv }),
                Target::Psp => {
                    // (v: repeats of the picture from -FACADE_V to FACADE_V over the unsigned range)
                    let v16 = ((w.uv[k][1] / pack::FACADE_V * 0.5 + 0.5).clamp(0.0, 1.0) * 65535.0).round() as u16;
                    let tint = [0, 1, 2].map(|c| (w.color[c] as u16 * ao as u16 / 255) as u8);
                    Raw::from(&[&(uv[0] as u16).to_le_bytes(), &v16.to_le_bytes(), &psp565(tint), &s16(pos[0]), &s16(pos[1]), &s16(pos[2])])
                }
                // (the sector the wall faces stands in for its normal: the program holds a light per sector)
                Target::Pica => Raw::from(&[&s16(pos[0]), &s16(pos[1]), &s16(pos[2]), &[ao, w.sector], &[w.color[0], w.color[1], w.color[2], w.gain], &uv[0].to_le_bytes(), &uv[1].to_le_bytes()]),
                // (a vertex program of OpenGL ES can take a byte apart: over the sector's five bits, three say how
                // late in the dusk the building's rooms come on)
                Target::Gles => Raw::from(&[&s16(pos[0]), &s16(pos[1]), &s16(pos[2]), &[ao, w.sector | (w.late >> 5) << 5], &[w.color[0], w.color[1], w.color[2], w.gain], &uv[0].to_le_bytes(), &uv[1].to_le_bytes()]),
            }
        });
        (v, w.p)
    }
    fn solid(&self, s: &SolidTri) -> ([Raw; 3], [P3; 3]) {
        let v = [0, 1, 2].map(|k| {
            let pos = [q_wide(s.p[k][0], self.x0, self.side), qy(s.p[k][1]), q_wide(s.p[k][2], self.z0, self.side)];
            let (ao, n) = (self.sky.open(s.p[k], s.n[k]), n8(s.n[k]));
            match self.target {
                Target::Vita | Target::Gles3 => Raw::of(&SolidVertex { pos, ao, pad: 0, normal: n, pad2: 0, color: s.color }),
                Target::Psp => {
                    let c = [0, 1, 2].map(|c| (s.color[c] as u16 * ao as u16 / 255) as u8);
                    Raw::from(&[&psp565(c), &s16(pos[0]), &s16(pos[1]), &s16(pos[2])])
                }
                Target::Pica | Target::Gles => Raw::from(&[&s16(pos[0]), &s16(pos[1]), &s16(pos[2]), &[ao, city::solid_sector(&s.p)], &s.color]),
            }
        });
        (v, s.p)
    }
}

/// What a tile's bundle gives the area.
struct Read {
    slot: usize,
    built: buildings::Built,
    /// PLATEAU's own models: bridges, street furniture.
    models: Vec<SolidTri>,
    /// Per building: its footprint rings (x, z) and the height of its top.
    footprints: Vec<(Vec<Vec<f32>>, f32)>,
    lamps: Vec<Lamp>,
    /// Trees: where each stands (x, ground, z), how tall and how wide its crown is.
    trees: Vec<[f32; 5]>,
}

/// A lamp's light on the ground: where it falls, how far it reaches, how high the lamp hangs, and its colour.
#[derive(Clone, Copy)]
struct Lamp {
    x: f32,
    z: f32,
    reach: f32,
    height: f32,
    color: [f32; 3],
}

/// A tile, compiled: the near level without its ground, the ground the near and mid levels share, mid and far.
#[derive(Default)]
struct TileOut {
    near: Level,
    ground: Vec<TopTri>,
    mid: Level,
    far: Level,
}

fn run() -> Result<(), String> {
    let t0 = Instant::now();
    let input = PathBuf::from(arg("--in").ok_or("--in <CityIR directory>")?);
    let out_dir = PathBuf::from(arg("--out").ok_or("--out <directory>")?);
    let profile = read_json(Path::new(&arg("--profile").ok_or("--profile <file>")?))?;
    let target = Target::of(profile["target"].as_str().unwrap_or("vita"))?;
    let manifest = ir::manifest(&input)?;
    let facade = Facade::from_json(&read_json(&input.join("facade.json"))?)?;
    let tile = manifest.tile;
    let f = |v: &Value, k: &str, i: usize, d: f32| v[k][i].as_f64().map(|x| x as f32).unwrap_or(d);
    let num = |v: &Value, k: &str, d: f32| v[k].as_f64().map(|x| x as f32).unwrap_or(d);
    // The tile grid, widened to whole regions: a region is `region` blocks, a block BLOCK_TILES tiles a side.
    let region_blocks = profile["region"].as_u64().unwrap_or(2) as usize;
    let per_region = BLOCK_TILES * region_blocks as i32;
    // A machine without fragment programs lights a group of faces with one colour: its solids are ordered by
    // sector as walls are. A handheld reads each cell's near level by itself.
    let faced = !target.resident();
    let streamed = !target.resident();
    // A landmark as a model of its own at each level of detail, drawn by its distance alone: the handhelds,
    // and a GPU that pays for every small triangle whatever it hides (a lattice tower of 19 000 triangles
    // two kilometres away is a few hundred pixels).
    let landmarked = streamed || target == Target::Gles3;
    let floor_to = |v: i32| v.div_euclid(per_region) * per_region;
    let (tx0, tz0) = (floor_to(manifest.tiles.iter().map(|t| t.x).min().ok_or("no tiles")?), floor_to(manifest.tiles.iter().map(|t| t.z).min().unwrap()));
    let (tx1, tz1) = (floor_to(manifest.tiles.iter().map(|t| t.x).max().unwrap()) + per_region - 1, floor_to(manifest.tiles.iter().map(|t| t.z).max().unwrap()) + per_region - 1);
    let (nx, nz) = ((tx1 - tx0 + 1) as usize, (tz1 - tz0 + 1) as usize);
    let slot = |t: (i32, i32)| -> Option<usize> { (t.0 >= tx0 && t.0 <= tx1 && t.1 >= tz0 && t.1 <= tz1).then(|| (t.1 - tz0) as usize * nx + (t.0 - tx0) as usize) };
    let (bnx, bnz) = (nx / BLOCK_TILES as usize, nz / BLOCK_TILES as usize);
    let (rnx, rnz) = (bnx / region_blocks, bnz / region_blocks);
    let block = tile * BLOCK_TILES as f32;
    let cells = profile["cells"].as_u64().unwrap_or(4) as usize;

    // Where a lamp hangs over its prop, from the model library: (street light height and reach forward, pole lamp
    // sideways and height, how many heights away its light still falls).
    let library = ir::Bundle::read(&input.join("models.cir"))?;
    let place = &library.meta["placement"];
    let placement = ([num(&place["lamp"], "y", 8.5), num(&place["lamp"], "z", 2.25)], [num(&place["poleLamp"], "x", 1.0), num(&place["poleLamp"], "y", 5.6)], num(place, "poolReach", 2.6));

    // The kinds of tree: height and crown radius of each (`TREES` in web/src/world/props.js).
    let kinds: Vec<[f32; 2]> = library.meta["trees"].as_array().map(|a| a.iter().map(|t| [num(t, "height", 9.0), num(t, "radius", 4.0)]).collect()).unwrap_or_default();

    // ---- the tiles' own triangles
    let read: Vec<Read> = manifest
        .tiles
        .par_iter()
        .map(|t| -> Result<_, String> {
            let b = ir::Bundle::read(&input.join(format!("tile_{}_{}.cir", t.x, t.z)))?;
            let built = buildings::build(&b, &facade, (t.x, t.z));
            let (pos, rgb) = (b.f32("models.position"), b.u8("models.rgb"));
            let mut models = Vec::with_capacity(pos.len() / 9);
            for (i, tri) in pos.chunks_exact(9).enumerate() {
                let p = [[tri[0], tri[1], tri[2]], [tri[3], tri[4], tri[5]], [tri[6], tri[7], tri[8]]];
                let (n, area) = face(p[0], p[1], p[2]);
                if area > 1e-5 {
                    models.push(SolidTri { p, n: [n; 3], color: [rgb[i * 9], rgb[i * 9 + 1], rgb[i * 9 + 2], 0] });
                }
            }
            let (info, rings, points) = (b.f32("foot.info"), b.u32("foot.rings"), b.f32("foot.points"));
            let footprints = info
                .chunks_exact(7)
                .map(|r| {
                    let (first, count) = (r[5] as usize, r[6] as usize);
                    let list = (first..first + count).map(|k| points[rings[k * 3] as usize * 2..(rings[k * 3] + rings[k * 3 + 1]) as usize * 2].to_vec()).collect();
                    (list, r[0] + r[1])
                })
                .collect();
            // Street lights, park lamps and the lamps on utility poles (`Props.build` in web/src/world/props.js).
            let mut lamps = Vec::new();
            let mut trees = Vec::new();
            let stand = b.f32("props.ground");
            for (k, row) in b.f32("props").chunks_exact(6).enumerate() {
                if row[0] as i32 == 0 && !kinds.is_empty() {
                    let kind = &kinds[row[1] as usize % kinds.len()];
                    trees.push([row[3], stand.get(k).copied().unwrap_or(0.0), row[4], kind[0] * row[5], kind[1] * 0.875 * row[5]]);
                }
            }
            for row in b.f32("props").chunks_exact(6) {
                let (kind, rot, x, z, scale) = (row[0] as i32, row[2], row[3], row[4], row[5]);
                let (sin, cos) = rot.sin_cos();
                let at = |lx: f32, lz: f32| (x + lx * cos + lz * sin, z - lx * sin + lz * cos);
                if kind == 2 {
                    let (px, pz) = at(0.0, placement.0[1] + 1.0);
                    let street = scale >= 1.0;
                    lamps.push(Lamp { x: px, z: pz, reach: placement.2 * placement.0[0] * if street { 1.0 } else { 0.7 }, height: placement.0[0] * if street { 1.0 } else { 0.6 }, color: if street { [1.9, 1.5, 0.95] } else { [0.51, 0.45, 0.32] } });
                } else if kind == 1 {
                    let (px, pz) = at(placement.1[0] + 0.6, 0.0);
                    lamps.push(Lamp { x: px, z: pz, reach: placement.2 * placement.1[1], height: placement.1[1], color: [1.15, 1.25, 1.4] });
                }
            }
            Ok(Read { slot: slot((t.x, t.z)).unwrap(), built, models, footprints, lamps, trees })
        })
        .collect::<Result<_, _>>()?;
    println!("cook: {} tiles read in {:.1} s", read.len(), t0.elapsed().as_secs_f32());

    // ---- every roof into one grid of heights, with whose roof it is
    let mut all: Vec<Building> = Vec::new();
    let mut roofs = prisms::RoofGrid::new(tx0 as f32 * tile, tz0 as f32 * tile, (tx1 + 1) as f32 * tile, (tz1 + 1) as f32 * tile, 1.0);
    for r in &read {
        let first = all.len() as u32;
        all.extend_from_slice(&r.built.buildings);
        for (t, owner) in &r.built.tops {
            roofs.raise(&t.p, if *owner == u32::MAX { 0 } else { first + owner + 1 });
        }
        for (k, (rings, top)) in r.footprints.iter().enumerate() {
            if r.built.buildings.get(k).is_some_and(|b| !b.lattice) {
                let rings: Vec<&[f32]> = rings.iter().map(|r| r.as_slice()).collect();
                roofs.footprint(&rings, first + k as u32 + 1, *top);
            }
        }
    }
    let rules: Vec<(usize, prisms::Rules)> = profile["prisms"]
        .as_array()
        .ok_or("the profile has no prisms")?
        .iter()
        .map(|p| (p["cell"].as_u64().unwrap_or(1) as usize, prisms::Rules { cap: num(p, "cap", 3.0), min_area: num(p, "minArea", 12.0), eps: num(p, "eps", 0.75), min_height: num(p, "minHeight", 1.5), smooth: p["smooth"].as_u64().unwrap_or(1) as u32 }))
        .collect();
    if rules.len() != LODS - 1 {
        return Err(format!("the profile needs {} prism levels", LODS - 1));
    }
    let coarse: Vec<prisms::RoofGrid> = rules.iter().map(|(cell, _)| roofs.coarser(*cell)).collect();
    // A machine that cannot draw the mesher's own triangles near the eye takes prisms there too.
    let near_rule: Option<(prisms::RoofGrid, prisms::Rules)> = profile["near"].as_object().map(|_| {
        let p = &profile["near"];
        (roofs.coarser(p["cell"].as_u64().unwrap_or(1) as usize), prisms::Rules { cap: num(p, "cap", 3.0), min_area: num(p, "minArea", 12.0), eps: num(p, "eps", 0.75), min_height: num(p, "minHeight", 1.5), smooth: p["smooth"].as_u64().unwrap_or(1) as u32 })
    });
    println!("cook: {} buildings, roofs drawn in {:.1} s", all.len(), t0.elapsed().as_secs_f32());

    // ---- every triangle to the tile it lies in
    let mut tiles: Vec<TileOut> = (0..nx * nz).map(|_| TileOut::default()).collect();
    let mut present = vec![false; nx * nz];
    // Painted geometry by tile: the lattice tower as it is, and what may be simplified.
    let mut lattice: Vec<Vec<SolidTri>> = vec![Vec::new(); nx * nz];

    let mut painted: Vec<Vec<SolidTri>> = vec![Vec::new(); nx * nz];
    let mut crowns: Vec<Vec<SolidTri>> = vec![Vec::new(); nx * nz];
    let put_solid = |to: &mut Vec<Vec<SolidTri>>, s: &SolidTri| {
        let tri: [V<6>; 3] = [0, 1, 2].map(|k| [s.p[k][0], s.p[k][1], s.p[k][2], s.n[k][0], s.n[k][1], s.n[k][2]]);
        by_tile(&tri, tile, |t, p| {
            if let Some(i) = slot(t) {
                to[i].push(SolidTri { p: p.map(|v| [v[0], v[1], v[2]]), n: p.map(|v| [v[3], v[4], v[5]]), color: s.color });
            }
        });
    };
    for r in &read {
        let home = r.slot;
        present[home] = true;
        for (t, _) in &r.built.tops {
            let tri: [V<6>; 3] = [0, 1, 2].map(|k| [t.p[k][0], t.p[k][1], t.p[k][2], t.n[k][0], t.n[k][1], t.n[k][2]]);
            by_tile(&tri, tile, |tl, p| {
                if let Some(i) = slot(tl) {
                    tiles[i].near.tops.push(TopTri { p: p.map(|v| [v[0], v[1], v[2]]), n: p.map(|v| [v[3], v[4], v[5]]) });
                }
            });
        }
        // (a wall belongs to the tile its middle lies in; its block's vertices reach a margin beyond the block)
        for w in &r.built.walls {
            if let Some(i) = slot(city::tile_of((w.p[0][0] + w.p[1][0] + w.p[2][0]) / 3.0, (w.p[0][2] + w.p[1][2] + w.p[2][2]) / 3.0, tile)) {
                tiles[i].near.walls.push(*w);
            }
        }
        for s in &r.built.solids {
            put_solid(&mut lattice, s);
        }

        for s in &r.models {
            put_solid(&mut painted, s);
        }
        // A tree near the eye: a crown of two rings of six between two tips, round to the light, on the tree's
        // place in the picture.
        for t in &r.trees {
            let seed = buildings::hash((t[0] * 16.0) as i32 as u32, (t[2] * 16.0) as i32 as u32);
            let vary = |shift: u32, lo: f32, hi: f32| lo + (hi - lo) * ((seed >> shift) & 255) as f32 / 255.0;
            let (cx, cy, cz, r, h) = (t[0], t[1] + t[3] * 0.62, t[2], t[4] * vary(8, 0.8, 1.05), t[3] * 0.38);
            let shade = vary(0, 0.8, 1.2);
            let color = [srgb8(0.035 * shade), srgb8(0.085 * shade), srgb8(0.022 * shade), 0];
            let turn = vary(16, 0.0, 1.0);
            let ring = |y: f32, k: usize| -> P3 {
                let a = (k as f32 + turn) * std::f32::consts::TAU / 6.0;
                [cx + a.cos() * r * 0.9, cy + y * h, cz + a.sin() * r * 0.9]
            };
            let (top, bottom) = ([cx, cy + h, cz], [cx, cy - h, cz]);
            let out = |p: P3| geom::norm([(p[0] - cx) / r, (p[1] - cy) / h, (p[2] - cz) / r]);
            let Some(i) = slot(city::tile_of(cx, cz, tile)) else { continue };
            for k in 0..6 {
                let (a, b, c, d) = (ring(0.42, k), ring(0.42, k + 1), ring(-0.42, k), ring(-0.42, k + 1));
                for p in [[a, top, b], [a, b, d], [a, d, c], [d, bottom, c]] {
                    // (seen from outside, counter-clockwise)
                    let (g, _) = face(p[0], p[1], p[2]);
                    let m = [(p[0][0] + p[1][0] + p[2][0]) / 3.0 - cx, (p[0][1] + p[1][1] + p[2][1]) / 3.0 - cy, (p[0][2] + p[1][2] + p[2][2]) / 3.0 - cz];
                    let p = if geom::dot(g, m) < 0.0 { [p[0], p[2], p[1]] } else { p };
                    crowns[i].push(SolidTri { p, n: p.map(out), color });
                }
            }
        }
    }
    // ---- railways, elevated roads and structures of the whole area
    let global = ir::Bundle::read(&input.join("global.cir"))?;
    for part in global.meta["parts"].as_array().cloned().unwrap_or_default() {
        let id = part["id"].as_str().unwrap_or("");
        let pos = global.f32(&format!("{id}.position"));
        let col = global.f32(&format!("{id}.color"));
        let base = [f(&part, "color", 0, 1.0), f(&part, "color", 1, 1.0), f(&part, "color", 2, 1.0)];
        // A textured part (the track bed) is drawn in the mean colour of ballast and sleepers.
        let tint = if part["map"] == Value::Bool(true) { [0.33, 0.31, 0.29] } else { [1.0, 1.0, 1.0] };
        for (i, tri) in pos.chunks_exact(9).enumerate() {
            let p = [[tri[0], tri[1], tri[2]], [tri[3], tri[4], tri[5]], [tri[6], tri[7], tri[8]]];
            let (n, area) = face(p[0], p[1], p[2]);
            if area < 1e-5 {
                continue;
            }
            let c = if col.is_empty() { [1.0, 1.0, 1.0] } else { [col[i * 9], col[i * 9 + 1], col[i * 9 + 2]] };
            // The rails themselves are seven centimetres of steel: from the air they are thinner than a pixel.
            if part["name"] == "rails" && !col.is_empty() && c[0] < 0.12 && c[0] > 0.05 && area < 3.0 {
                continue;
            }
            let color = [srgb8(c[0] * base[0] * tint[0]), srgb8(c[1] * base[1] * tint[1]), srgb8(c[2] * base[2] * tint[2]), 0];
            put_solid(&mut painted, &SolidTri { p, n: [n; 3], color });
        }
    }
    let lamps: Vec<Lamp> = read.iter().flat_map(|r| r.lamps.iter().copied()).collect();
    drop(read);

    // ---- the ground and the simple levels of detail, tile by tile
    // The ground of a tile within an error. A flap hangs down each edge that lies on the border of a place
    // `places` tiles wide: it hides the gap to a neighbouring place at another level of detail.
    let ground_of = |ground: &mut terrain::Ground, i: usize, level: usize, places: i32| -> Vec<TopTri> {
        let (tx, tz) = ((i % nx) as i32, (i / nx) as i32);
        let (x0, z0) = ((tx + tx0) as f32 * tile, (tz + tz0) as f32 * tile);
        let mut tris = ground.mesh(f(&profile["terrain"], "error", level, 1.0), profile["terrain"]["stride"][level].as_u64().unwrap_or(4) as usize);
        let skirt = f(&profile["terrain"], "skirt", level, 5.0);
        let mut out = Vec::with_capacity(tris.len() + 64);
        for t in &mut tris {
            terrain::face_up(t);
            let n = t.map(|p| manifest.surface.normal(p[0], p[2]));
            out.push(TopTri { p: *t, n });
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let on = |k: usize, v: f32| (a[k] - v).abs() < 1e-3 && (b[k] - v).abs() < 1e-3;
                let border = (on(0, x0) && tx % places == 0) || (on(0, x0 + tile) && tx % places == places - 1) || (on(2, z0) && tz % places == 0) || (on(2, z0 + tile) && tz % places == places - 1);
                if border {
                    let (a2, b2) = ([a[0], a[1] - skirt, a[2]], [b[0], b[1] - skirt, b[2]]);
                    let (na, nb) = (n[e], n[(e + 1) % 3]);
                    out.push(TopTri { p: [a, a2, b], n: [na, na, nb] });
                    out.push(TopTri { p: [b, a2, b2], n: [nb, na, nb] });
                }
            }
        }
        out
    };
    let simple: Vec<TileOut> = (0..nx * nz)
        .into_par_iter()
        .map(|i| {
            let mut out = TileOut::default();
            let (x0, z0) = (((i % nx) as i32 + tx0) as f32 * tile, ((i / nx) as i32 + tz0) as f32 * tile);
            // (a tile the export does not have is open ground: the far level keeps a floor under the haze)
            let mut ground = terrain::Ground::new(&manifest.surface, x0, z0, tile);
            out.far.tops = ground_of(&mut ground, i, 1, per_region);
            if !present[i] {
                return out;
            }
            out.ground = ground_of(&mut ground, i, 0, BLOCK_TILES);
            for (lod, level) in [&mut out.mid, &mut out.far].into_iter().enumerate() {
                let p = prisms::prisms(&coarse[lod], &manifest.surface, x0, z0, tile, &rules[lod].1, &all, &facade);
                level.tops.extend(p.tops);
                level.walls.extend(p.walls);
            }
            if let Some((grid, rule)) = &near_rule {
                let p = prisms::prisms(grid, &manifest.surface, x0, z0, tile, rule, &all, &facade);
                out.near.tops = p.tops;
                out.near.walls = p.walls;
            }
            // Painted geometry: all of it near, less of it in the middle; far away it is in the region's picture.
            out.near.solids = simplify::solids(&painted[i], f(&profile["solids"], "error", 0, 0.15), f(&profile["solids"], "minSize", 0, 0.0));
            out.mid.solids = simplify::solids(&painted[i], f(&profile["solids"], "error", 1, 1.0), f(&profile["solids"], "minSize", 1, 0.0));
            // (elsewhere a lattice tower is a landmark, drawn from `LAND`)
            if !landmarked {
                for level in [&mut out.near, &mut out.mid, &mut out.far] {
                    level.solids.extend_from_slice(&lattice[i]);
                }
            }
            if near_rule.is_none() {
                out.near.solids.extend_from_slice(&crowns[i]);
            }
            out
        })
        .collect();
    let mut heights = heights::Heights::new(tx0 as f32 * tile, tz0 as f32 * tile, (tx1 + 1) as f32 * tile, (tz1 + 1) as f32 * tile, profile["heights"]["step"].as_f64().unwrap_or(2.0) as f32, Y0);
    for (i, s) in simple.into_iter().enumerate() {
        for t in s.ground.iter().chain(&tiles[i].near.tops) {
            heights.raise(&t.p);
        }
        for s in &s.near.solids {
            heights.raise(&s.p);
        }
        if !present[i] {
            for t in &s.far.tops {
                heights.raise(&t.p);
            }
        }
        let TileOut { near, ground, mid, far } = s;
        if near_rule.is_some() {
            tiles[i].near.tops = near.tops;
            tiles[i].near.walls = near.walls;
        }
        tiles[i].near.solids = near.solids;
        tiles[i].ground = ground;
        tiles[i].mid = mid;
        tiles[i].far = far;
    }
    // ---- lamp light on the ground: per cell of the height grid the brightest lamp that reaches it, as
    // r5 g6 b5 at half strength. Nothing falls on a roof.
    let mut lamp_light = vec![[0.0f32; 3]; heights.w * heights.h];
    for l in &lamps {
        let edge = (1.0 + (l.reach / l.height).powi(2)).powf(-1.5);
        let cells = |v: f32, o: f32, n: usize| ((((v - l.reach - o) / heights.step).floor().max(0.0)) as usize, ((((v + l.reach - o) / heights.step).ceil()) as usize).min(n));
        let ((i0, i1), (j0, j1)) = (cells(l.x, heights.x0, heights.w), cells(l.z, heights.z0, heights.h));
        for j in j0..j1 {
            for i in i0..i1 {
                let (x, z) = (heights.x0 + (i as f32 + 0.5) * heights.step, heights.z0 + (j as f32 + 0.5) * heights.step);
                let d2 = (x - l.x).powi(2) + (z - l.z).powi(2);
                // The light a lamp throws straight down, thinned by distance and slant; zero at its reach.
                let v = (((1.0 + d2 / (l.height * l.height)).powf(-1.5) - edge) / (1.0 - edge)).max(0.0);
                let cell = &mut lamp_light[j * heights.w + i];
                for k in 0..3 {
                    cell[k] = cell[k].max(l.color[k] * v);
                }
            }
        }
    }
    let lamp_map: Vec<u16> = (0..heights.w * heights.h)
        .map(|o| {
            let (i, j) = (o % heights.w, o / heights.w);
            let (x, z) = (heights.x0 + (i as f32 + 0.5) * heights.step, heights.z0 + (j as f32 + 0.5) * heights.step);
            if heights.data[o] - manifest.surface.sample(x, z) > 3.5 {
                return 0;
            }
            // An ordered dither hides the steps of five and six bits in the dim skirts of the pools.
            let d = [[0.0, 0.5], [0.75, 0.25]][j & 1][i & 1] - 0.375;
            let q = |v: f32, levels: f32| ((v * 0.5).clamp(0.0, 1.0) * levels + d).round().clamp(0.0, levels) as u16;
            let c = lamp_light[o];
            (q(c[0], 31.0) << 11) | (q(c[1], 63.0) << 5) | q(c[2], 31.0)
        })
        .collect();
    println!("cook: {} lamps", lamps.len());
    let c: Vec<usize> = prisms::COUNT.iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).collect();
    println!("cook: levels of detail in {:.1} s ({} terraces, {} cell edges, {} straightened, {} walls)", t0.elapsed().as_secs_f32(), c[0], c[1], c[2], c[3]);

    // ---- draws: regions, blocks, cells
    let mut out = Out::default();
    let mut regions = vec![Region::default(); rnx * rnz];
    let mut blocks = vec![Block::default(); bnx * bnz];
    let mut cell_table: Vec<Cell> = Vec::with_capacity(bnx * bnz * cells * cells);
    // Handheld packs: per cell the bytes of its near level (top, wall and solid vertices, indices).
    let mut near_parts: Vec<[Vec<u8>; 4]> = Vec::new();
    // Bytes a cell's geometry may take: what a slot of the device holds, less the cell's picture.
    let cell_room = profile["near"]["room"].as_u64().unwrap_or(u64::MAX) as usize;
    let mut coarsened = 0;
    let range = |ys: &mut (f32, f32), b: &Bounds| {
        ys.0 = ys.0.min(b.min[1]);
        ys.1 = ys.1.max(b.max[1]);
    };
    // A level's batches with their indices by cell (and, for walls, by sector within the cell).
    let by_cell = |out: &mut Out, level: &mut Level, frame: Frame, lod: usize, ys: &mut (f32, f32)| -> Draws {
        let side = frame.side / cells as f32;
        let cell_of = |p: &[P3; 3]| -> usize {
            let cx = (((p[0][0] + p[1][0] + p[2][0]) / 3.0 - frame.x0) / side).floor().clamp(0.0, cells as f32 - 1.0) as usize;
            let cz = (((p[0][2] + p[1][2] + p[2][2]) / 3.0 - frame.z0) / side).floor().clamp(0.0, cells as f32 - 1.0) as usize;
            cz * cells + cx
        };
        level.tops.sort_by_key(|t| cell_of(&t.p));
        level.walls.sort_by_key(|w| cell_of(&w.p) * (SECTORS + 1) + w.sector as usize);
        let solid_group = |s: &SolidTri| if faced { cell_of(&s.p) * (SECTORS + 1) + city::solid_sector(&s.p) as usize } else { cell_of(&s.p) };
        level.solids.sort_by_key(solid_group);
        let first = out.batches.len();
        for c in chunks(&level.tops, |t| frame.top(t)) {
            let at = store(&mut out.vtop, &c.vertices);
            let group: Vec<usize> = c.source.iter().map(|&k| cell_of(&level.tops[k].p)).collect();
            out.batch(kind::TOP, at, c.vertices.len(), &c.bounds, &c.tris, Some((&group, cells * cells)));
            out.stats[lod][0] += c.tris.len();
            range(ys, &c.bounds);
        }
        for c in chunks(&level.walls, |w| frame.wall(w)) {
            let at = store(&mut out.vwal, &c.vertices);
            let group: Vec<usize> = c.source.iter().map(|&k| cell_of(&level.walls[k].p) * (SECTORS + 1) + level.walls[k].sector as usize).collect();
            out.batch(kind::WALL, at, c.vertices.len(), &c.bounds, &c.tris, Some((&group, cells * cells * (SECTORS + 1))));
            out.stats[lod][1] += c.tris.len();
            range(ys, &c.bounds);
        }
        for c in chunks(&level.solids, |s| frame.solid(s)) {
            let at = store(&mut out.vsol, &c.vertices);
            let group: Vec<usize> = c.source.iter().map(|&k| solid_group(&level.solids[k])).collect();
            out.batch(kind::SOLID, at, c.vertices.len(), &c.bounds, &c.tris, Some((&group, cells * cells * if faced { SECTORS + 1 } else { 1 })));
            out.stats[lod][2] += c.tris.len();
            range(ys, &c.bounds);
        }
        Draws { first: first as u32, count: (out.batches.len() - first) as u32 }
    };
    // The far level: a region's tiles together.
    for (r, region) in regions.iter_mut().enumerate() {
        let (rx, rz) = (r % rnx, r / rnx);
        let frame = Frame { x0: (tx0 + rx as i32 * per_region) as f32 * tile, z0: (tz0 + rz as i32 * per_region) as f32 * tile, side: tile * per_region as f32, sky: &heights, target };
        let mut level = Level::default();
        for tz in 0..per_region as usize {
            for tx in 0..per_region as usize {
                let far = std::mem::take(&mut tiles[(rz * per_region as usize + tz) * nx + rx * per_region as usize + tx].far);
                level.tops.extend(far.tops);
                level.walls.extend(far.walls);
                level.solids.extend(far.solids);
            }
        }
        let mut ys = (f32::INFINITY, f32::NEG_INFINITY);
        let far = by_cell(&mut out, &mut level, frame, 2, &mut ys);
        *region = Region { y_min: ys.0 - 20.0, y_max: ys.1, far, ..Default::default() };
    }
    // The mid level by block and the near level by cell.
    for (b, record) in blocks.iter_mut().enumerate() {
        let (bx, bz) = (b % bnx, b / bnx);
        let frame = Frame { x0: (tx0 + bx as i32 * BLOCK_TILES) as f32 * tile, z0: (tz0 + bz as i32 * BLOCK_TILES) as f32 * tile, side: block, sky: &heights, target };
        let side = block / cells as f32;
        let cell_of = |p: &[P3; 3]| -> usize {
            let cx = (((p[0][0] + p[1][0] + p[2][0]) / 3.0 - frame.x0) / side).floor().clamp(0.0, cells as f32 - 1.0) as usize;
            let cz = (((p[0][2] + p[1][2] + p[2][2]) / 3.0 - frame.z0) / side).floor().clamp(0.0, cells as f32 - 1.0) as usize;
            cz * cells + cx
        };
        let mut near: Vec<Level> = (0..cells * cells).map(|_| Level::default()).collect();
        let mut mid = Level::default();
        for tz in 0..BLOCK_TILES as usize {
            for tx in 0..BLOCK_TILES as usize {
                let t = std::mem::take(&mut tiles[(bz * BLOCK_TILES as usize + tz) * nx + bx * BLOCK_TILES as usize + tx]);
                for g in &t.ground {
                    near[cell_of(&g.p)].tops.push(*g);
                }
                mid.tops.extend(t.ground);
                for x in t.near.tops {
                    near[cell_of(&x.p)].tops.push(x);
                }
                for x in t.near.walls {
                    near[cell_of(&x.p)].walls.push(x);
                }
                for x in t.near.solids {
                    near[cell_of(&x.p)].solids.push(x);
                }
                mid.tops.extend(t.mid.tops);
                mid.walls.extend(t.mid.walls);
                mid.solids.extend(t.mid.solids);
            }
        }
        let mut ys = (f32::INFINITY, f32::NEG_INFINITY);
        let mid = by_cell(&mut out, &mut mid, frame, 1, &mut ys);
        // ---- near, cell by cell
        for level in &mut near {
            let first = out.batches.len();
            let mut bounds = Bounds::new();
            if faced {
                level.walls.sort_by_key(|w| w.sector);
            }
            // A handheld reads a cell by itself, into a slot of a fixed size: its vertices and indices count from
            // the cell's own start, and painted geometry that would not fit is simplified until it does.
            let held = streamed.then(|| (std::mem::take(&mut out.vtop), std::mem::take(&mut out.vwal), std::mem::take(&mut out.vsol), std::mem::take(&mut out.idx)));
            let before = (out.spans.len(), out.stats[0]);
            for attempt in 0.. {
                if faced {
                    level.solids.sort_by_key(|s| city::solid_sector(&s.p));
                }
                bounds = Bounds::new();
                for c in chunks(&level.tops, |t| frame.top(t)) {
                    let at = store(&mut out.vtop, &c.vertices);
                    out.batch(kind::TOP, at, c.vertices.len(), &c.bounds, &c.tris, None);
                    out.stats[0][0] += c.tris.len();
                    bounds.add(&[c.bounds.min, c.bounds.max]);
                }
                for c in chunks(&level.walls, |w| frame.wall(w)) {
                    let at = store(&mut out.vwal, &c.vertices);
                    let group: Vec<usize> = c.source.iter().map(|&k| level.walls[k].sector as usize).collect();
                    out.batch(kind::WALL, at, c.vertices.len(), &c.bounds, &c.tris, faced.then_some((&group[..], SECTORS + 1)));
                    out.stats[0][1] += c.tris.len();
                    bounds.add(&[c.bounds.min, c.bounds.max]);
                }
                for c in chunks(&level.solids, |s| frame.solid(s)) {
                    let at = store(&mut out.vsol, &c.vertices);
                    let group: Vec<usize> = c.source.iter().map(|&k| city::solid_sector(&level.solids[k].p) as usize).collect();
                    out.batch(kind::SOLID, at, c.vertices.len(), &c.bounds, &c.tris, faced.then_some((&group[..], SECTORS + 1)));
                    out.stats[0][2] += c.tris.len();
                    bounds.add(&[c.bounds.min, c.bounds.max]);
                }
                let bytes = out.vtop.len() + out.vwal.len() + out.vsol.len() + out.idx.len() * 2;
                if !streamed || bytes <= cell_room || attempt >= 5 || level.solids.is_empty() {
                    break;
                }
                // Over: the same cell again with its painted geometry twice as coarse.
                let coarse = 2.0f32.powi(attempt + 1);
                level.solids = simplify::solids(&level.solids, f(&profile["solids"], "error", 0, 0.15) * coarse, f(&profile["solids"], "minSize", 0, 0.0) * coarse);
                coarsened += 1;
                out.batches.truncate(first);
                out.spans.truncate(before.0);
                out.stats[0] = before.1;
                out.vtop.clear();
                out.vwal.clear();
                out.vsol.clear();
                out.idx.clear();
            }
            if let Some(h) = held {
                let idx = std::mem::replace(&mut out.idx, h.3);
                near_parts.push([std::mem::replace(&mut out.vtop, h.0), std::mem::replace(&mut out.vwal, h.1), std::mem::replace(&mut out.vsol, h.2), pack::slice_bytes(&idx).to_vec()]);
            }
            if bounds.min[1] <= bounds.max[1] {
                range(&mut ys, &bounds);
            }
            cell_table.push(Cell { min: bounds.min, max: bounds.max, near: Draws { first: first as u32, count: (out.batches.len() - first) as u32 } });
        }
        if ys.1 > Y0 + Y_SPAN {
            return Err(format!("a point at {} m is above the height range", ys.1));
        }
        *record = Block { y_min: ys.0 - 20.0, y_max: ys.1, mid, ..Default::default() };
    }
    drop(tiles);
    // ---- a handheld's landmarks: a model at each level of detail, made from the members the export recorded,
    // in the frame of the block the landmark stands in
    let marks = if landmarked { landmark::read(&input.join("landmarks.cir"))? } else { Vec::new() };
    // The reference's own triangles of what is a landmark: the nearest level of a machine that keeps normals.
    let own: Vec<SolidTri> = if target == Target::Gles3 { lattice.iter().flatten().copied().collect() } else { Vec::new() };
    let mark_rule = |lod: usize| landmark::Rule { rank: 2 - lod as u8, thick: f(&profile["landmarks"], "thick", lod, [0.0, 1.6, 3.2][lod]), lamps: f(&profile["landmarks"], "lamps", lod, 0.0) };
    let mut landmarks: Vec<Landmark> = Vec::new();
    let mut mark_meta: Vec<Value> = Vec::new();
    let open_sky = heights::Heights { x0: 0.0, z0: 0.0, step: 1.0, w: 1, h: 1, data: vec![f32::NEG_INFINITY] };
    for l in &marks {
        let (bx, bz) = ((((l.at[0] - tx0 as f32 * tile) / block) as usize).min(bnx - 1), (((l.at[1] - tz0 as f32 * tile) / block) as usize).min(bnz - 1));
        let frame = Frame { x0: tx0 as f32 * tile + bx as f32 * block, z0: tz0 as f32 * tile + bz as f32 * block, side: block, sky: &open_sky, target };
        let mut record = Landmark { min: [f32::INFINITY; 3], max: [f32::NEG_INFINITY; 3], reach: [f(&profile["landmarks"], "reach", 0, 450.0), f(&profile["landmarks"], "reach", 1, 1100.0)], block: (bz * bnx + bx) as u32, ..Default::default() };
        let mut counts = [0usize; LODS];
        for lod in 0..LODS {
            let mut tris = if lod == 0 && !own.is_empty() {
                // Near the eye: the reference's triangles within the landmark's reach, and its lamps alone
                // from the members.
                let near = |s: &&SolidTri| (s.p[0][0] - l.at[0]).hypot(s.p[0][2] - l.at[1]) < 260.0;
                let lamps = landmark::Landmark { name: l.name.clone(), at: l.at, beams: Vec::new(), boxes: Vec::new(), lamps: l.lamps.iter().map(|x| landmark::Lamp { at: x.at, size: x.size, color: x.color }).collect() };
                own.iter().filter(near).copied().chain(landmark::model(&lamps, mark_rule(lod), false)).collect()
            } else {
                landmark::model(l, mark_rule(lod), lod == 0)
            };
            tris.sort_by_key(|s| city::solid_sector(&s.p));
            let first = out.batches.len();
            for c in chunks(&tris, |s| frame.solid(s)) {
                let at = store(&mut out.vsol, &c.vertices);
                let group: Vec<usize> = c.source.iter().map(|&k| city::solid_sector(&tris[k].p) as usize).collect();
                out.batch(kind::OPEN, at, c.vertices.len(), &c.bounds, &c.tris, faced.then_some((&group[..], SECTORS + 1)));
                counts[lod] += c.tris.len();
                for k in 0..3 {
                    record.min[k] = record.min[k].min(c.bounds.min[k]);
                    record.max[k] = record.max[k].max(c.bounds.max[k]);
                }
            }
            record.lods[lod] = Draws { first: first as u32, count: (out.batches.len() - first) as u32 };
        }
        mark_meta.push(json!({"model": l.name, "members": l.beams.len(), "triangles": counts}));
        landmarks.push(record);
    }
    println!("cook: {} landmarks {}, {} cells with coarser painted geometry to fit", landmarks.len(), serde_json::to_string(&mark_meta).unwrap(), coarsened);
    let sizes = target.sizes();
    println!("cook: {} draws, {} + {} + {} vertices, {} indices in {:.1} s", out.batches.len(), out.vtop.len() / sizes[0], out.vwal.len() / sizes[1], out.vsol.len() / sizes[2], out.idx.len(), t0.elapsed().as_secs_f32());

    // ---- pictures
    let cache = tex::Cache::new(PathBuf::from(arg("--cache").unwrap_or(".pocket-build/cache/bc".into())));
    let floor = profile["ground"]["floor"].as_u64().unwrap_or(8) as usize;
    let block_top = profile["ground"]["block"].as_u64().unwrap_or(1024) as usize;
    let far_top = profile["ground"]["far"].as_u64().unwrap_or(512) as usize;
    // A place's picture, composed of its tiles' pictures: `None` where the export has none.
    let compose = |name: &str, first: (i32, i32), tiles_side: usize, size: usize| -> Result<Option<tex::Image>, String> {
        let part = size / tiles_side;
        let mut img = tex::Image::blank(size, size, [111, 110, 104]);
        let mut any = false;
        for tz in 0..tiles_side {
            for tx in 0..tiles_side {
                let path = input.join(format!("{name}_{}_{}.png", first.0 + tx as i32, first.1 + tz as i32));
                if !path.exists() {
                    continue;
                }
                let mut t = tex::load(&path)?;
                while t.w > part {
                    t = t.half();
                }
                img.blit(&t, tx * part, tz * part);
                any = true;
            }
        }
        Ok(any.then_some(img))
    };
    let block_first = |b: usize| (tx0 + (b % bnx) as i32 * BLOCK_TILES, tz0 + (b / bnx) as i32 * BLOCK_TILES);
    let region_first = |r: usize| (tx0 + (r % rnx) as i32 * per_region, tz0 + (r / rnx) as i32 * per_region);
    let mut glvl: Vec<GroundLevel> = Vec::new();
    let mut gtex: Vec<u8> = Vec::new();
    let (mut facd, mut facn) = (Vec::new(), Vec::new());
    let mut hpic: Vec<pack::HandPicture> = Vec::new();
    let mut htex: Vec<u8> = Vec::new();
    let mut ncel: Vec<NearCell> = Vec::new();
    let mut near: Vec<u8> = Vec::new();
    // What a machine that keeps the whole city reads its pictures as: (colour, colour with a mask).
    let formats = if target == Target::Gles3 { (pack::tex_format::ETC2, pack::tex_format::ETC2A) } else { (pack::tex_format::BC1, pack::tex_format::BC3) };
    if target.resident() {
        let chain = |img: Option<tex::Image>| -> Vec<(usize, Vec<u8>)> {
            // (open ground beyond the export: one flat level)
            let mut img = img.unwrap_or_else(|| tex::Image::blank(floor, floor, [111, 110, 104]));
            let mut levels = Vec::new();
            loop {
                levels.push((img.w, cache.compress(&img, formats.0)));
                if img.w <= floor {
                    break;
                }
                img = img.half();
            }
            levels
        };
        let block_pictures: Vec<Vec<(usize, Vec<u8>)>> = (0..bnx * bnz).into_par_iter().map(|b| compose("top", block_first(b), BLOCK_TILES as usize, block_top).map(chain)).collect::<Result<_, _>>()?;
        let region_pictures: Vec<Vec<(usize, Vec<u8>)>> = (0..rnx * rnz).into_par_iter().map(|r| compose("far", region_first(r), per_region as usize, far_top).map(chain)).collect::<Result<_, _>>()?;
        let mut place = |levels: &[(usize, Vec<u8>)]| -> (u32, u32) {
            let first = glvl.len() as u32;
            for (w, bytes) in levels {
                glvl.push(GroundLevel { offset: gtex.len() as u32, size: bytes.len() as u32, width: *w as u32, pad: 0 });
                gtex.extend_from_slice(bytes);
            }
            (first, levels.len() as u32)
        };
        for (b, levels) in block_pictures.iter().enumerate() {
            (blocks[b].ground_first, blocks[b].ground_levels) = place(levels);
        }
        for (r, levels) in region_pictures.iter().enumerate() {
            (regions[r].ground_first, regions[r].ground_levels) = place(levels);
        }
        let mips = profile["facade"]["mips"].as_u64().unwrap_or(6) as u32;
        facd = cache.chain(tex::load(&input.join("facade.day.png"))?, formats.1, mips);
        facn = cache.chain(tex::load(&input.join("facade.night.png"))?, formats.0, mips);
    } else {
        // A handheld takes every picture as a day and night pair. A block's picture shows the structures the
        // mid and far levels do not draw (the far pictures of the export); a cell's own picture, read with its
        // near level, is the ground as the near level stands on it.
        let lamp_at = |x: f32, z: f32| -> [f32; 3] {
            let (fx, fz) = ((x - heights.x0) / heights.step - 0.5, (z - heights.z0) / heights.step - 0.5);
            let (i, j) = (fx.floor(), fz.floor());
            let (u, v) = (fx - i, fz - j);
            let at = |i: f32, j: f32| -> [f32; 3] {
                let (i, j) = ((i as isize).clamp(0, heights.w as isize - 1) as usize, (j as isize).clamp(0, heights.h as isize - 1) as usize);
                if lamp_map[j * heights.w + i] == 0 {
                    [0.0; 3]
                } else {
                    lamp_light[j * heights.w + i]
                }
            };
            let (a, b, c, d) = (at(i, j), at(i + 1.0, j), at(i, j + 1.0), at(i + 1.0, j + 1.0));
            [0, 1, 2].map(|k| (a[k] * (1.0 - u) + b[k] * u) * (1.0 - v) + (c[k] * (1.0 - u) + d[k] * u) * v)
        };
        let pair = |day: tex::Image, x0: f32, z0: f32, side: f32, most: usize| -> (Vec<u8>, usize, usize) {
            let night = handheld::night_ground(&day, |u, v| lamp_at(x0 + u * side, z0 + v * side));
            let w = day.w;
            // (the ground's pictures leave half the palette to the device, for its shadows)
            let (bytes, levels) = handheld::pair(target, handheld::Kind::Ground, day, night, most);
            (bytes, w, levels)
        };
        let block_pictures: Vec<(Vec<u8>, usize, usize)> = (0..bnx * bnz)
            .into_par_iter()
            .map(|b| {
                let first = block_first(b);
                compose("far", first, BLOCK_TILES as usize, block_top).map(|i| pair(i.unwrap_or_else(|| tex::Image::blank(16, 16, [111, 110, 104])), first.0 as f32 * tile, first.1 as f32 * tile, block, usize::MAX))
            })
            .collect::<Result<_, _>>()?;
        for (bytes, w, levels) in &block_pictures {
            hpic.push(pack::HandPicture { offset: htex.len() as u32, size: bytes.len() as u32, width: *w as u16, height: *w as u16, levels: *levels as u32 });
            htex.extend_from_slice(bytes);
            htex.resize((htex.len() + 15) & !15, 0);
        }
        let size = [profile["facade"]["size"][0].as_u64().unwrap_or(512) as usize, profile["facade"]["size"][1].as_u64().unwrap_or(128) as usize];
        let (day, night) = handheld::facades(&tex::load(&input.join("facade.day.png"))?, &tex::load(&input.join("facade.night.png"))?, size[0], size[1], 0.7354, target != Target::Psp);
        let (bytes, levels) = handheld::pair(target, handheld::Kind::Facade, day, night, usize::MAX);
        hpic.push(pack::HandPicture { offset: htex.len() as u32, size: bytes.len() as u32, width: size[0] as u16, height: size[1] as u16, levels: levels as u32 });
        htex.extend_from_slice(&bytes);

        // ---- the cells: a tile's picture cut by cell
        let cell_size = profile["ground"]["cell"].as_u64().unwrap_or(256) as usize;
        let cell_levels = profile["ground"]["cellLevels"].as_u64().unwrap_or(3) as usize;
        let per_tile = cells / BLOCK_TILES as usize;
        if per_tile * BLOCK_TILES as usize != cells {
            return Err("a handheld pack needs whole cells in a tile".into());
        }
        let cell_side = block / cells as f32;
        let mut cell_pictures: Vec<Option<(Vec<u8>, usize, usize)>> = (0..nx * nz)
            .into_par_iter()
            .map(|i| -> Result<Vec<(usize, (Vec<u8>, usize, usize))>, String> {
                let (tx, tz) = (i % nx, i / nx);
                let (bx, bz) = (tx / BLOCK_TILES as usize, tz / BLOCK_TILES as usize);
                let ids: Vec<(usize, usize, usize)> = (0..per_tile * per_tile)
                    .map(|k| (k % per_tile, k / per_tile))
                    .map(|(kx, kz)| (kx, kz, (bz * bnx + bx) * cells * cells + ((tz % BLOCK_TILES as usize) * per_tile + kz) * cells + (tx % BLOCK_TILES as usize) * per_tile + kx))
                    .filter(|c| near_parts[c.2].iter().any(|p| !p.is_empty()))
                    .collect();
                if ids.is_empty() {
                    return Ok(Vec::new());
                }
                let path = input.join(format!("top_{}_{}.png", tx0 + tx as i32, tz0 + tz as i32));
                let img = if path.exists() { tex::load(&path)? } else { tex::Image::blank(cell_size * per_tile, cell_size * per_tile, [111, 110, 104]) };
                let part = img.w / per_tile;
                Ok(ids
                    .into_iter()
                    .map(|(kx, kz, id)| {
                        let mut crop = tex::Image::blank(part, part, [0; 3]);
                        for y in 0..part {
                            let from = ((kz * part + y) * img.w + kx * part) * 4;
                            crop.rgba[y * part * 4..(y + 1) * part * 4].copy_from_slice(&img.rgba[from..from + part * 4]);
                        }
                        let day = handheld::resize(&crop, cell_size.min(part), cell_size.min(part));
                        (id, pair(day, (tx0 + tx as i32) as f32 * tile + kx as f32 * cell_side, (tz0 + tz as i32) as f32 * tile + kz as f32 * cell_side, cell_side, cell_levels))
                    })
                    .collect())
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .fold((0..near_parts.len()).map(|_| None).collect(), |mut all: Vec<Option<(Vec<u8>, usize, usize)>>, (id, p)| {
                all[id] = Some(p);
                all
            });
        for (id, parts) in near_parts.iter().enumerate() {
            let Some((picture, w, levels)) = cell_pictures[id].take() else {
                ncel.push(NearCell::default());
                continue;
            };
            let offset = near.len();
            let mut sizes = [0u32; 5];
            for (k, part) in parts.iter().map(|p| p.as_slice()).chain([picture.as_slice()]).enumerate() {
                sizes[k] = part.len() as u32;
                near.extend_from_slice(part);
                near.resize((near.len() + 15) & !15, 0);
            }
            ncel.push(NearCell { offset: offset as u32, size: (near.len() - offset) as u32, parts: sizes, width: w as u16, levels: levels as u16 });
        }
    }
    println!("cook: pictures in {:.1} s ({:.1} MB of ground, {:.1} MB of cells)", t0.elapsed().as_secs_f32(), (gtex.len() + htex.len()) as f32 / 1e6, near.len() as f32 / 1e6);

    // What the area says about itself: where the camera starts and the places of its tour.
    let area = arg("--area").map(|p| read_json(Path::new(&p))).transpose()?.unwrap_or(Value::Null);
    let six = |v: &Value| -> [f32; 6] { core::array::from_fn(|k| v[k].as_f64().unwrap_or(0.0) as f32) };
    let view = if area["view"].is_array() { six(&area["view"]) } else { [620.0, 260.0, 760.0, 0.0, 110.0, 0.0] };
    let tour: Vec<[f32; 6]> = area["tour"].as_array().map(|a| a.iter().map(six).collect()).unwrap_or_default();
    // ---- the traffic's lanes: each side of each road, on its own line (`buildGraph` in web/src/world/traffic.js)
    let roads = read_json(&input.join("roads.json"))?;
    let mut lanes: Vec<tokyo_sim::traffic::Lane> = Vec::new();
    let mut lane_points: Vec<[f32; 3]> = Vec::new();
    for e in roads["edges"].as_array().cloned().unwrap_or_default() {
        let highway = e["highway"].as_str().unwrap_or("");
        let count = e["lanes"].as_i64().unwrap_or(1);
        // (driveways and yards carry no through traffic; what runs in a tunnel is not seen)
        if (highway == "service" && count <= 1) || e["tunnel"].as_i64().unwrap_or(0) != 0 {
            continue;
        }
        let pts: Vec<[f32; 3]> = e["pts"].as_array().map(|a| a.chunks_exact(3).map(|c| [c[0].as_f64().unwrap_or(0.0) as f32, c[1].as_f64().unwrap_or(0.0) as f32, c[2].as_f64().unwrap_or(0.0) as f32]).collect()).unwrap_or_default();
        if pts.len() < 2 {
            continue;
        }
        let minor = ["residential", "unclassified", "living_street", "service"].contains(&highway);
        let speed = (e["maxspeed"].as_f64().unwrap_or(40.0) as f32 * if minor { 0.6 } else { 0.8 } / 3.6).clamp(4.5, 15.0);
        let weight = if highway.starts_with("motorway") { 5.0 } else if ["trunk", "primary"].contains(&highway) { 4.0 } else if ["secondary", "tertiary"].contains(&highway) { 2.5 } else { 1.0 };
        let oneway = e["oneway"].as_i64().unwrap_or(0);
        let (a, b) = (e["a"].as_u64().unwrap_or(0) as u32, e["b"].as_u64().unwrap_or(0) as u32);
        for forward in [true, false] {
            if (forward && oneway == -1) || (!forward && oneway == 1) {
                continue;
            }
            let line: Vec<[f32; 3]> = if forward { pts.clone() } else { pts.iter().rev().copied().collect() };
            let n = (if oneway != 0 { count.max(1) } else { (count / 2).max(1) }).min(3) as usize;
            for k in 0..n {
                // To the right of travel: a one-way road's lanes about its line, a two-way road's on its left half.
                let offset = if oneway != 0 { (k as f32 - (n as f32 - 1.0) / 2.0) * 3.0 } else if count >= 2 { -(k as f32 + 0.5) * 3.0 } else { -1.15 };
                let first = lane_points.len() as u32;
                let mut length = 0.0f32;
                for i in 0..line.len() {
                    let (p, q) = (line[i.saturating_sub(1)], line[(i + 1).min(line.len() - 1)]);
                    let (dx, dz) = (q[0] - p[0], q[2] - p[2]);
                    let l = (dx * dx + dz * dz).sqrt().max(1e-4);
                    let at = [line[i][0] - dz / l * offset, line[i][1] + 0.12, line[i][2] + dx / l * offset];
                    if i > 0 {
                        let prev = lane_points[lane_points.len() - 1];
                        length += ((at[0] - prev[0]).powi(2) + (at[1] - prev[1]).powi(2) + (at[2] - prev[2]).powi(2)).sqrt();
                    }
                    lane_points.push(at);
                }
                if length < 1.0 {
                    lane_points.truncate(first as usize);
                    continue;
                }
                lanes.push(tokyo_sim::traffic::Lane { first, count: line.len() as u32, from: if forward { a } else { b }, to: if forward { b } else { a }, speed, length, weight: weight / n as f32, pad: 0 });
            }
        }
    }
    println!("cook: {} lanes of traffic, {:.0} km", lanes.len(), lanes.iter().map(|l| l.length).sum::<f32>() / 1000.0);
    let unit = profile["heights"]["unit"].as_f64().unwrap_or(1.0 / 128.0) as f32;
    // The heights as the pack keeps them: every `store`-th cell each way, the highest of what it covers.
    let keep = profile["heights"]["store"].as_u64().unwrap_or(1) as usize;
    let (gw, gh) = (heights.w / keep, heights.h / keep);
    let hmap: Vec<u16> = (0..gw * gh)
        .map(|o| {
            let (i, j) = (o % gw, o / gw);
            let mut top = f32::NEG_INFINITY;
            for jj in 0..keep {
                for ii in 0..keep {
                    top = top.max(heights.data[(j * keep + jj) * heights.w + i * keep + ii]);
                }
            }
            ((top - Y0) / unit).round().clamp(0.0, 65535.0) as u16
        })
        .collect();
    let city = City {
        block,
        blocks_x: bnx as u32,
        blocks_z: bnz as u32,
        x0: tx0 as f32 * tile,
        z0: tz0 as f32 * tile,
        cells: cells as u32,
        y0: Y0,
        y_span: Y_SPAN,
        margin: MARGIN,
        grid_x0: heights.x0,
        grid_z0: heights.z0,
        grid_step: heights.step * keep as f32,
        grid_w: gw as u32,
        grid_h: gh as u32,
        height_step: unit,
        view,
        hour: num(&area, "hour", 15.5),
        region_blocks: region_blocks as u32,
        flags: if faced { flag::FACED } else { 0 } | if streamed { flag::STREAMED } else { 0 } | if target == Target::Gles3 { flag::ETC2 | flag::LANDMARKS } else { 0 },
    };
    let name = |l: usize| ["near", "mid", "far"][l];
    let meta = json!({
        "name": manifest.json["name"], "area": manifest.json["area"], "profile": profile["name"],
        "source": {"compiled": manifest.json["compiled"], "attribution": manifest.json["attribution"]},
        "tiles": present.iter().filter(|p| **p).count(), "blocks": bnx * bnz, "regions": rnx * rnz, "cells": cell_table.len(),
        "buildings": all.len(),
        "triangles": (0..LODS).map(|l| (name(l).to_string(), json!({"top": out.stats[l][0], "wall": out.stats[l][1], "solid": out.stats[l][2]}))).collect::<serde_json::Map<_, _>>(),
        "draws": out.batches.len(),
        "landmarks": mark_meta,
        "bytes": {"top": out.vtop.len(), "wall": out.vwal.len(), "solid": out.vsol.len(), "index": out.idx.len() * 2, "ground": gtex.len() + htex.len(), "facade": facd.len() + facn.len(), "heights": hmap.len() * 2, "cells": near.len(), "largestCell": ncel.iter().map(|c| c.size).max().unwrap_or(0)},
    });
    let mut w = pack::Writer::default();
    w.add(pack::META, serde_json::to_vec(&meta).unwrap());
    w.add(pack::CITY, pack::bytes_of(&city).to_vec());
    w.add(pack::REGN, pack::slice_bytes(&regions).to_vec());
    w.add(pack::BLCK, pack::slice_bytes(&blocks).to_vec());
    w.add(pack::CELL, pack::slice_bytes(&cell_table).to_vec());
    w.add(pack::BTCH, pack::slice_bytes(&out.batches).to_vec());
    w.add(pack::SPAN, pack::slice_bytes(&out.spans).to_vec());
    w.add(pack::VTOP, out.vtop);
    w.add(pack::VWAL, out.vwal);
    w.add(pack::VSOL, out.vsol);
    w.add(pack::IDX0, pack::slice_bytes(&out.idx).to_vec());
    w.add(pack::HMAP, pack::slice_bytes(&hmap).to_vec());
    if target.resident() {
        w.add(pack::FACD, facd);
        w.add(pack::FACN, facn);
        w.add(pack::LAMP, pack::slice_bytes(&lamp_map).to_vec());
        if target == Target::Gles3 {
            w.add(pack::LAND, pack::slice_bytes(&landmarks).to_vec());
        }
    } else {
        if target != Target::Psp {
            // The lamps' light as a texture over the stored grid of heights, 1 024 texels a side: the 3DS and
            // the iPod touch add it to the ground by night.
            let side = 1024;
            if gw > side || gh > side {
                return Err("the grid of heights is wider than the texture of the lamps' light".into());
            }
            let mut texels = vec![0u16; side * side];
            for j in 0..gh {
                for i in 0..gw {
                    let mut sum = [0.0f32; 3];
                    for jj in 0..keep {
                        for ii in 0..keep {
                            let o = (j * keep + jj) * heights.w + i * keep + ii;
                            if lamp_map[o] != 0 {
                                for k in 0..3 {
                                    sum[k] += lamp_light[o][k];
                                }
                            }
                        }
                    }
                    let q = |v: f32, levels: f32| ((v / (keep * keep) as f32 * 0.5).clamp(0.0, 1.0) * levels).round() as u16;
                    texels[j * side + i] = (q(sum[0], 31.0) << 11) | (q(sum[1], 63.0) << 5) | q(sum[2], 31.0);
                }
            }
            w.add(pack::LAMP, if target == Target::Pica { handheld::pica_tile(&texels, side, side) } else { pack::slice_bytes(&texels).to_vec() });
        }
        w.add(pack::HPIC, pack::slice_bytes(&hpic).to_vec());
        w.add(pack::HTEX, htex);
        w.add(pack::NCEL, pack::slice_bytes(&ncel).to_vec());
        w.add(pack::LAND, pack::slice_bytes(&landmarks).to_vec());
    }
    w.add(pack::TOUR, pack::slice_bytes(&tour).to_vec());
    w.add(pack::LANE, pack::slice_bytes(&lanes).to_vec());
    w.add(pack::LPTS, pack::slice_bytes(&lane_points).to_vec());
    if target.resident() {
        w.add(pack::GLVL, pack::slice_bytes(&glvl).to_vec());
        w.add(pack::GTEX, gtex);
    } else {
        // (last: a device keeps the rest in memory and reads this a cell at a time)
        w.add(pack::NEAR, near);
    }
    let bytes = w.finish();
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("city.pack"), &bytes).map_err(|e| e.to_string())?;
    let sha: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    let receipt = json!({"pack": {"bytes": bytes.len(), "sha256": sha}, "meta": meta, "seconds": t0.elapsed().as_secs_f32()});
    std::fs::write(out_dir.join("receipt.json"), serde_json::to_vec_pretty(&receipt).unwrap()).map_err(|e| e.to_string())?;
    println!("cook: {} ({:.1} MB) in {:.1} s", out_dir.join("city.pack").display(), bytes.len() as f32 / 1e6, t0.elapsed().as_secs_f32());
    println!("{}", serde_json::to_string_pretty(&meta["triangles"]).unwrap());
    Ok(())
}
