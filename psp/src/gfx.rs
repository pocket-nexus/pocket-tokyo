//! The GE renderer.
//!
//! A frame is one display list, built while the GE draws it:
//!
//! 1. the sky, a dome coloured at its vertices;
//! 2. the near pass: the cells around the eye, each with the picture of its
//!    own ground, with a short frustum and three quarters of the 16-bit depth
//!    buffer;
//! 3. the far pass: the mid and the far level, with a frustum from where they
//!    start to the horizon and the other quarter.
//!
//! A landmark is drawn in whichever of the two passes its distance puts it,
//! or in both, from its own model for that distance.
//!
//! The GE has no programs. What the Vita's do is done with what it has:
//!
//! - **Day and night are two palettes of one picture.** Every picture is 8-bit
//!   indices with a palette for the day and one for the night; the palette the
//!   GE reads is their mix by the hour, made here when the hour has moved.
//! - **Light is one colour per group of faces.** No vertex has a normal. The
//!   pack orders walls and painted geometry by the sector of the compass they
//!   face, so a draw is one sector, and the GE's ambient colour for that draw
//!   is the sun and the sky on a wall that looks that way. The ground and the
//!   roofs are one group. The vertex's own colour (what it sees of the sky,
//!   and a wall's tint) multiplies it.
//! - **The ground's texture coordinates are its position**, through the GE's
//!   texture matrix: a ground vertex is a colour and three coordinates.
//! - **Haze** is the GE's fog.

use core::ffi::c_void;
use core::ptr;

use alloc::vec::Vec;
use psp::sys::*;
use psp::Align16;
use tokyo_pack::{self as pack, kind, Batch, Block, Cell, City, HandPicture, Landmark, Region, KINDS, SECTORS};
use tokyo_sim::flight::Flight;
use tokyo_sim::mat::{self, Mat4};
use tokyo_sim::math::*;
use tokyo_sim::sky;
use tokyo_sim::text;
use tokyo_sim::view::{self, Item};

use crate::shade::{self, Ground};
use crate::store;
use crate::stream::Streamer;

const LIST_WORDS: usize = 98_304;
static mut LIST: Align16<[u32; LIST_WORDS]> = Align16([0; LIST_WORDS]);
/// One white texel, for what is painted: the texture stage doubles a colour only when there is a texture.
static mut WHITE: Align16<[u16; 64]> = Align16([0xffff; 64]);

/// The letters: indices into `INK`, drawn once at the start.
static mut LETTERS: Align16<[u8; text::ATLAS_W * text::ATLAS_H]> = Align16([0; text::ATLAS_W * text::ATLAS_H]);
/// Nothing, and white.
static mut INK: Align16<[u32; 16]> = Align16([0, 0xffff_ffff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
/// Letters a frame may draw, each twice: its shadow, then itself.
const TEXT_MOST: usize = 160;

/// A corner of a letter's quad, in the screen's own coordinates.
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct TextVertex {
    u: u16,
    v: u16,
    color: u32,
    x: i16,
    y: i16,
    z: i16,
    pad: u16,
}

/// One frame buffer: 512 × 272 texels of 16 bits. The colour and depth buffers are all this size.
pub const FB_BYTES: usize = 512 * 272 * 2;
/// The far pass's share of the depth buffer.
const DEPTH_SPLIT: i32 = 16384;
/// Day of the year the sun follows (early October).
const DAY: f32 = 277.0;
/// Steps of the mix between a picture's day and night palettes.
const STEPS: f32 = 64.0;
/// Palettes mixed in one frame at most.
const MIX_BUDGET: u32 = 10;

const SKY_SEGMENTS: usize = 16;
/// Sines of the dome's rings' heights, from below the horizon up; the zenith is one more vertex.
const SKY_RINGS: [f32; 8] = [-0.34, -0.10, 0.0, 0.09, 0.21, 0.41, 0.67, 0.91];
const SKY_VERTS: usize = SKY_SEGMENTS * SKY_RINGS.len() + 1;
const SKY_INDICES: usize = (SKY_RINGS.len() - 1) * SKY_SEGMENTS * 6 + SKY_SEGMENTS * 3;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct SkyVertex {
    color: u32,
    pos: [f32; 3],
}

fn vtype_top() -> VertexType {
    VertexType::COLOR_5650 | VertexType::VERTEX_16BIT | VertexType::INDEX_16BIT | VertexType::TRANSFORM_3D
}
fn vtype_wall() -> VertexType {
    VertexType::TEXTURE_16BIT | VertexType::COLOR_5650 | VertexType::VERTEX_16BIT | VertexType::INDEX_16BIT | VertexType::TRANSFORM_3D
}
fn vtype_text() -> VertexType {
    VertexType::TEXTURE_16BIT | VertexType::COLOR_8888 | VertexType::VERTEX_16BIT | VertexType::TRANSFORM_2D
}
fn vtype_sky() -> VertexType {
    VertexType::COLOR_8888 | VertexType::VERTEX_32BITF | VertexType::INDEX_16BIT | VertexType::TRANSFORM_3D
}

fn abgr(c: [f32; 3]) -> u32 {
    let b = |x: f32| (clamp(x, 0.0, 1.0) * 255.0 + 0.5) as u32;
    0xff00_0000 | (b(c[2]) << 16) | (b(c[1]) << 8) | b(c[0])
}

fn fmatrix(m: &Mat4) -> ScePspFMatrix4 {
    let col = |c: usize| ScePspFVector4 { x: m[c], y: m[4 + c], z: m[8 + c], w: m[12 + c] };
    ScePspFMatrix4 { x: col(0), y: col(1), z: col(2), w: col(3) }
}

fn vec4(x: f32, y: f32, z: f32, w: f32) -> ScePspFVector4 {
    ScePspFVector4 { x, y, z, w }
}

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub draws: u32,
    pub tris: [u32; KINDS],
    /// Triangles of the near, the mid and the far level.
    pub level: [u32; 3],
    /// Cells near, blocks mid, blocks far.
    pub places: [u32; 3],
    pub turned: u32,
    pub binds: u32,
    pub mixes: u32,
    /// Microseconds choosing the draws, and writing the list.
    pub select: u32,
    pub list: u32,
    /// Words of the display list used.
    pub words: u32,
}

/// What a draw last set, so that the next sets only what differs.
struct Last {
    frame: u32,
    sector: u32,
    vtx: *const u8,
}

pub struct Gfx {
    pub city: City,
    regions: Vec<Region>,
    blocks: Vec<Block>,
    pub cells: Vec<Cell>,
    batches: Vec<Batch>,
    spans: Vec<u32>,
    vertices: [Vec<u8>; 3],
    idx: Vec<u16>,
    pictures: Vec<HandPicture>,
    htex: Vec<u8>,
    /// Per picture of `pictures`: its palette as the hour mixes it, and the step that mix holds.
    mixed: Vec<u32>,
    held: Vec<u32>,
    _shade: shade::Shade,
    /// Where the eye is, and the frame's counts, between choosing a frame's draws and writing its list.
    chosen: (f32, u32),
    pub heights: Vec<u16>,
    landmarks: Vec<Landmark>,
    /// Where the far pass starts and the near pass ends, metres from the eye.
    split: (f32, f32),
    lists: [Vec<Item>; KINDS],
    sky_vb: Vec<SkyVertex>,
    sky_dirs: Vec<V3>,
    sky_ib: Vec<u16>,
    text_vb: Vec<TextVertex>,
    /// Corners written this frame.
    text_at: usize,
    pub stats: Stats,
    pub resident_bytes: usize,
    /// Development switches (`option=` on the control line): 1 no culling, 2 the other winding, 4 one light for
    /// every sector, 8 no fog, 16 no lighting stage, 32 no texture, 64 nearest texel, 128 no clipping.
    pub option: u32,
}

/// What a mixed palette depends on: the step between day and night, and how much of each colour a shadow leaves.
#[derive(Clone, Copy)]
struct Hour {
    step: u32,
    shade: [u32; 3],
}

impl Hour {
    fn key(&self, shaded: bool) -> u32 {
        if shaded {
            self.step << 15 | (self.shade[0] >> 3) << 10 | (self.shade[1] >> 3) << 5 | self.shade[2] >> 3
        } else {
            self.step << 15
        }
    }
}

/// The palette the GE reads for a picture this frame: `out`, mixed again when the hour has moved a step.
/// `shaded`: the picture uses the lower half of its palette, and the upper half is that half in shadow.
unsafe fn mix(day: *const u32, out: *mut u32, held: &mut u32, hour: &Hour, shaded: bool, budget: &mut u32) {
    let key = hour.key(shaded);
    if *held == key || (*held != u32::MAX && *budget == 0) {
        return;
    }
    *budget = budget.saturating_sub(1);
    *held = key;
    let night = day.add(256);
    let (t, s) = (hour.step, STEPS as u32 - hour.step);
    for i in 0..if shaded { 128 } else { 256 } {
        let (a, b) = (*day.add(i), *night.add(i));
        let rb = (((a & 0x00ff_00ff) * s + (b & 0x00ff_00ff) * t) >> 6) & 0x00ff_00ff;
        let g = (((a & 0x0000_ff00) * s + (b & 0x0000_ff00) * t) >> 6) & 0x0000_ff00;
        *out.add(i) = (a & 0xff00_0000) | rb | g;
        if shaded {
            let (r, g, b) = (rb & 0xff, g >> 8, rb >> 16);
            *out.add(i + 128) = (a & 0xff00_0000) | ((b * hour.shade[2]) >> 8) << 16 | ((g * hour.shade[1]) >> 8) << 8 | (r * hour.shade[0]) >> 8;
        }
    }
    sceKernelDcacheWritebackRange(out as *const c_void, 1024);
}

/// Binds a picture of 8-bit indices: its palette, then its levels (`first` being the largest, at `data`).
unsafe fn bind(palette: *const u32, data: *const u8, width: u32, height: u32, levels: u32) {
    const LEVELS: [MipmapLevel; 8] = [MipmapLevel::None, MipmapLevel::Level1, MipmapLevel::Level2, MipmapLevel::Level3, MipmapLevel::Level4, MipmapLevel::Level5, MipmapLevel::Level6, MipmapLevel::Level7];
    sceGuClutMode(ClutPixelFormat::Psm8888, 0, 0xff, 0);
    sceGuClutLoad(32, palette as *const c_void);
    sceGuTexMode(TexturePixelFormat::PsmT8, levels as i32 - 1, 0, 1);
    let mut at = data;
    for l in 0..levels {
        let (w, h) = ((width >> l) as i32, (height >> l) as i32);
        sceGuTexImage(LEVELS[l as usize], w, h, w, at as *const c_void);
        at = at.add((w * h) as usize);
    }
}

impl Gfx {
    /// Reads what stays in memory and sets the GE up.
    pub unsafe fn load(file: &store::PackFile, progress: &mut dyn FnMut(&str)) -> Result<Gfx, &'static str> {
        progress("tables");
        let city: City = *file.records::<City>(pack::CITY)?.first().ok_or("city section")?;
        if city.region_blocks != 1 || city.flags & (pack::flag::FACED | pack::flag::STREAMED) != (pack::flag::FACED | pack::flag::STREAMED) || city.cells > 8 {
            return Err("the pack is not a PSP pack");
        }
        let regions: Vec<Region> = file.records(pack::REGN)?;
        let blocks: Vec<Block> = file.records(pack::BLCK)?;
        let cells: Vec<Cell> = file.records(pack::CELL)?;
        let batches: Vec<Batch> = file.records(pack::BTCH)?;
        let spans: Vec<u32> = file.records(pack::SPAN)?;
        progress("geometry");
        let vertices: [Vec<u8>; 3] = [file.records(pack::VTOP)?, file.records(pack::VWAL)?, file.records(pack::VSOL)?];
        let idx: Vec<u16> = file.records(pack::IDX0)?;
        let heights: Vec<u16> = file.records(pack::HMAP)?;
        progress("pictures");
        let pictures: Vec<HandPicture> = file.records(pack::HPIC)?;
        let mut htex: Vec<u8> = file.records(pack::HTEX)?;
        if pictures.len() < blocks.len() + 1 || htex.as_ptr() as usize & 15 != 0 {
            return Err("the pack's pictures");
        }
        let mixed = alloc::vec![0u32; pictures.len() * 256];
        let held = alloc::vec![u32::MAX; pictures.len()];
        progress("shadows");
        let grounds: Vec<Ground> = (0..blocks.len())
            .map(|b| {
                let p = pictures[b];
                Ground { data: htex.as_mut_ptr().add(p.offset as usize + 2048), width: p.width as u32, levels: p.levels, x0: city.x0 + (b % city.blocks_x as usize) as f32 * city.block, z0: city.z0 + (b / city.blocks_x as usize) as f32 * city.block, side: city.block }
            })
            .collect();
        let shade = shade::start(&heights, &city, grounds)?;
        let landmarks: Vec<Landmark> = file.records(pack::LAND)?;

        // The sky's dome: unit directions, and the triangles between its rings.
        let mut sky_dirs = Vec::with_capacity(SKY_VERTS);
        for y in SKY_RINGS {
            let r = sqrt(1.0 - y * y);
            for k in 0..SKY_SEGMENTS {
                let a = k as f32 * TAU / SKY_SEGMENTS as f32;
                sky_dirs.push(v3(cos(a) * r, y, sin(a) * r));
            }
        }
        sky_dirs.push(V3::UP);
        let mut sky_ib = Vec::with_capacity(SKY_INDICES);
        let n = SKY_SEGMENTS as u16;
        for ring in 0..SKY_RINGS.len() as u16 - 1 {
            for k in 0..n {
                let (a, b, c, d) = (ring * n + k, ring * n + (k + 1) % n, (ring + 1) * n + k, (ring + 1) * n + (k + 1) % n);
                sky_ib.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        let top = (SKY_RINGS.len() as u16 - 1) * n;
        for k in 0..n {
            sky_ib.extend_from_slice(&[top + k, top + n, top + (k + 1) % n]);
        }
        text::atlas(&mut *ptr::addr_of_mut!(LETTERS.0));
        let resident_bytes = vertices.iter().map(|v| v.len()).sum::<usize>() + idx.len() * 2 + htex.len() + heights.len() * 2 + mixed.len() * 4 + (batches.len() + cells.len()) * 48 + spans.len() * 4;
        sceKernelDcacheWritebackAll();

        sceGuInit();
        sceGuStart(GuContextType::Direct, ptr::addr_of_mut!(LIST.0) as *mut c_void);
        // 16-bit colour with ordered dither: half the memory traffic of 32-bit per pixel written.
        sceGuDrawBuffer(DisplayPixelFormat::Psm5650, ptr::null_mut(), 512);
        sceGuDispBuffer(480, 272, FB_BYTES as *mut c_void, 512);
        sceGuDepthBuffer((FB_BYTES * 2) as *mut c_void, 512);
        sceGuOffset(2048 - 240, 2048 - 136);
        sceGuViewport(2048, 2048, 480, 272);
        sceGuDepthRange(65535, 0);
        sceGuDepthFunc(DepthFunc::GreaterOrEqual);
        sceGuScissor(0, 0, 480, 272);
        sceGuEnable(GuState::ScissorTest);
        sceGuEnable(GuState::ClipPlanes);
        sceGuFrontFace(FrontFaceDirection::CounterClockwise);
        sceGuShadeModel(ShadingModel::Smooth);
        let row = |x, y, z, w| ScePspIVector4 { x, y, z, w };
        sceGuSetDither(&ScePspIMatrix4 { x: row(-4, 0, -3, 1), y: row(2, -2, 3, -1), z: row(-3, 1, -4, 0), w: row(3, -1, 2, -2) });
        sceGuEnable(GuState::Dither);
        sceGuBlendFunc(BlendOp::Add, BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha, 0, 0);
        // Light: no lamps of the GE's own; the ambient colour of a draw times the vertex's colour.
        sceGuLightMode(LightMode::SingleColor);
        sceGuColorMaterial(LightComponent::AMBIENT);
        sceGuModelColor(0, 0x00ff_ffff, 0, 0);
        sceGuMaterial(LightComponent::AMBIENT, 0xffff_ffff);
        sceGuFinish();
        sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
        sceDisplayWaitVblankStart();
        sceGuDisplay(true);

        Ok(Gfx {
            city,
            regions,
            blocks,
            cells,
            batches,
            spans,
            vertices,
            idx,
            pictures,
            htex,
            mixed,
            held,
            _shade: shade,
            chosen: (f32::MAX, 0),
            heights,
            landmarks,
            split: (0.0, 0.0),
            lists: core::array::from_fn(|_| Vec::with_capacity(1024)),
            sky_vb: alloc::vec![SkyVertex::default(); SKY_VERTS],
            sky_dirs,
            sky_ib,
            text_vb: alloc::vec![TextVertex::default(); TEXT_MOST * 4],
            text_at: 0,
            stats: Stats::default(),
            resident_bytes,
            option: 0,
        })
    }

    /// The top of whatever stands at a point, metres.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let c = &self.city;
        let i = (((x - c.grid_x0) / c.grid_step) as i32).clamp(0, c.grid_w as i32 - 1) as usize;
        let j = (((z - c.grid_z0) / c.grid_step) as i32).clamp(0, c.grid_h as i32 - 1) as usize;
        c.y0 + self.heights[j * c.grid_w as usize + i] as f32 * c.height_step
    }

    unsafe fn sky(&mut self, light: &sky::Light) {
        let glow = saturate(light.sun_dir.y * 6.0 + 0.6);
        let around = [(0.6 + light.sun[0]) * glow, (0.45 + light.sun[1]) * glow, (0.3 + light.sun[2]) * glow];
        for (v, d) in self.sky_vb.iter_mut().zip(&self.sky_dirs) {
            let up = sqrt(saturate(d.y));
            // Below the horizon lies the land beyond the city, under the same haze.
            let down = saturate(-d.y * 3.0);
            let s = saturate(d.dot(light.sun_dir));
            let s8 = {
                let s2 = s * s;
                let s4 = s2 * s2;
                s4 * s4
            };
            let s64 = {
                let a = s8 * s8;
                let b = a * a;
                b * b
            };
            let sun = (s8 * 0.18 + s64 * 0.35) * (1.0 - down);
            let c = [0, 1, 2].map(|k| lerp(light.horizon[k], light.zenith[k], up) * (1.0 - 0.3 * down) + around[k] * sun);
            v.color = abgr(c);
            v.pos = [d.x * 900.0, d.y * 900.0, d.z * 900.0];
        }
        sceKernelDcacheWritebackRange(self.sky_vb.as_ptr() as *const c_void, (SKY_VERTS * core::mem::size_of::<SkyVertex>()) as u32);
        sceGuDisable(GuState::DepthTest);
        sceGuDisable(GuState::Texture2D);
        sceGuDisable(GuState::Lighting);
        sceGuDisable(GuState::Fog);
        sceGuDisable(GuState::CullFace);
        sceGuDepthMask(1);
        let id = ScePspFMatrix4 { x: vec4(1.0, 0.0, 0.0, 0.0), y: vec4(0.0, 1.0, 0.0, 0.0), z: vec4(0.0, 0.0, 1.0, 0.0), w: vec4(0.0, 0.0, 0.0, 1.0) };
        sceGuSetMatrix(MatrixMode::Model, &id);
        sceGuDrawArray(GuPrimitive::Triangles, vtype_sky(), SKY_INDICES as i32, self.sky_ib.as_ptr() as *const c_void, self.sky_vb.as_ptr() as *const c_void);
        sceGuDepthMask(0);
        self.stats.draws += 1;
    }

    /// The draws of one kind in one pass: `near` for the cells' own level.
    #[allow(clippy::too_many_arguments)]
    unsafe fn pass(&mut self, k: usize, near: bool, eye: V3, streamer: &mut Streamer, lights: &[u32; SECTORS + 1], hour: &Hour, budget: &mut u32) {
        let c = self.city;
        let size = [8usize, 12, 8, 8][k];
        // (a landmark's members are painted geometry: the solids' vertices)
        let source = if k == kind::OPEN as usize { kind::SOLID as usize } else { k };
        let mut last = Last { frame: u32::MAX, sector: u32::MAX, vtx: ptr::null() };
        let mut bound = u32::MAX;
        let n = c.cells;
        // A position is a signed 16-bit number over its place: the GE reads it as -1..1.
        const HALF: f32 = 32768.0 / 65535.0;
        let mut since_stall = 0;
        for i in 0..self.lists[k].len() {
            let item = self.lists[k][i];
            let b = self.batches[item.batch as usize];
            if k == kind::OPEN as usize {
                // A landmark belongs to the pass its distance puts it in: to both when it stands across the two.
                let from = mat::box_distance(eye, &b.min, &b.max);
                let through = sqrt((b.max[0] - b.min[0]) * (b.max[0] - b.min[0]) + (b.max[1] - b.min[1]) * (b.max[1] - b.min[1]) + (b.max[2] - b.min[2]) * (b.max[2] - b.min[2]));
                if if near { from >= self.split.1 } else { from + through <= self.split.0 } {
                    continue;
                }
            } else if (item.cell != u32::MAX) != near {
                continue;
            }
            // ---- where its vertices are, and the picture of its ground
            let (vtx, idx) = if item.cell != u32::MAX {
                let (slot, rec) = streamer.slot(item.cell as usize);
                let part = |p: usize| rec.parts[..p].iter().map(|s| (*s as usize + 15) & !15).sum::<usize>();
                if k == kind::TOP as usize && bound != item.cell {
                    bound = item.cell;
                    let picture = slot.mem.add(part(4));
                    mix(picture as *const u32, slot.mixed, &mut slot.held, hour, true, budget);
                    bind(slot.mixed, picture.add(2048), rec.width as u32, rec.width as u32, rec.levels as u32);
                    // The cell's picture covers the cell: the block's coordinates, times the cells in a block.
                    let (cx, cz) = ((item.cell % (n * n)) % n, (item.cell % (n * n)) / n);
                    let s = HALF * n as f32;
                    let m = ScePspFMatrix4 { x: vec4(s, 0.0, 0.0, 0.0), y: vec4(0.0, 0.0, 0.0, 0.0), z: vec4(0.0, s, 0.0, 0.0), w: vec4(s - cx as f32, s - cz as f32, 1.0, 0.0) };
                    sceGuSetMatrix(MatrixMode::Texture, &m);
                    self.stats.binds += 1;
                }
                (slot.mem.add(part(k)).add(b.vtx_first as usize * size) as *const u8, slot.mem.add(part(3)).add(b.idx_first as usize * 2) as *const u16)
            } else {
                if k == kind::TOP as usize && bound != item.place {
                    bound = item.place;
                    let p = self.pictures[item.place as usize];
                    let data = self.htex.as_ptr().add(p.offset as usize);
                    let out = self.mixed.as_mut_ptr().add(item.place as usize * 256);
                    mix(data as *const u32, out, &mut self.held[item.place as usize], hour, true, budget);
                    bind(out, data.add(2048), p.width as u32, p.height as u32, p.levels);
                    let m = ScePspFMatrix4 { x: vec4(HALF, 0.0, 0.0, 0.0), y: vec4(0.0, 0.0, 0.0, 0.0), z: vec4(0.0, HALF, 0.0, 0.0), w: vec4(HALF, HALF, 1.0, 0.0) };
                    sceGuSetMatrix(MatrixMode::Texture, &m);
                    self.stats.binds += 1;
                }
                (self.vertices[source].as_ptr().add(b.vtx_first as usize * size), self.idx.as_ptr().add(b.idx_first as usize))
            };
            // ---- its place, as the eye sees it
            let frame = item.place << 1 | item.region as u32;
            if last.frame != frame {
                last.frame = frame;
                let (origin, span) = view::frame_of(&c, item.place, item.region, k == kind::TOP as usize);
                let s = [span[0] * HALF, span[1] * HALF, span[2] * HALF];
                let m = ScePspFMatrix4 { x: vec4(s[0], 0.0, 0.0, 0.0), y: vec4(0.0, s[1], 0.0, 0.0), z: vec4(0.0, 0.0, s[2], 0.0), w: vec4(origin[0] + s[0] - eye.x, origin[1] + s[1] - eye.y, origin[2] + s[2] - eye.z, 1.0) };
                sceGuSetMatrix(MatrixMode::Model, &m);
            }
            let sector = (item.sector as u32).min(SECTORS as u32);
            if last.sector != sector {
                last.sector = sector;
                sceGuAmbient(lights[sector as usize]);
            }
            if last.vtx != vtx {
                last.vtx = vtx;
                sceGuSendCommandi(GeCommand::Base, ((vtx as u32 >> 8) & 0xf0000) as i32);
                sceGuSendCommandi(GeCommand::Vaddr, (vtx as u32 & 0x00ff_ffff) as i32);
            }
            // ---- the draw: at most 65 535 indices in one
            let (mut from, to) = (item.from, item.to);
            while from < to {
                let count = (to - from).min(65_532);
                let at = idx.add(from as usize);
                since_stall += 1;
                if since_stall >= 24 {
                    // (this one also tells the GE how far the list has grown)
                    since_stall = 0;
                    sceGuDrawArray(GuPrimitive::Triangles, VertexType::empty(), count as i32, at as *const c_void, ptr::null());
                } else {
                    sceGuSendCommandi(GeCommand::Base, ((at as u32 >> 8) & 0xf0000) as i32);
                    sceGuSendCommandi(GeCommand::Iaddr, (at as u32 & 0x00ff_ffff) as i32);
                    sceGuSendCommandi(GeCommand::Prim, ((GuPrimitive::Triangles as i32) << 16) | count as i32);
                }
                from += count;
                self.stats.draws += 1;
                self.stats.tris[k] += count / 3;
                self.stats.level[if item.cell != u32::MAX { 0 } else if item.region { 2 } else { 1 }] += count / 3;
            }
        }
    }

    /// A line of text for this frame, its top left corner at `(x, y)`, each dot `scale` pixels.
    pub fn text(&mut self, x: i32, y: i32, scale: i32, color: u32, line: &str) {
        for (pass, tint) in [(1, 0xc000_0000u32), (0, color)] {
            for (k, c) in line.bytes().enumerate() {
                if c == b' ' || self.text_at + 2 > self.text_vb.len() {
                    continue;
                }
                let (u, v) = text::glyph(c);
                let (px, py) = (x + (k * text::ADVANCE) as i32 * scale + pass * scale.max(1), y + pass * scale.max(1));
                self.text_vb[self.text_at] = TextVertex { u: u as u16, v: v as u16, color: tint, x: px as i16, y: py as i16, z: 0, pad: 0 };
                self.text_vb[self.text_at + 1] = TextVertex { u: (u + 5) as u16, v: (v + 7) as u16, color: tint, x: (px + 5 * scale) as i16, y: (py + 7 * scale) as i16, z: 0, pad: 0 };
                self.text_at += 2;
            }
        }
    }

    /// The frame's text, over everything.
    unsafe fn letters(&mut self) {
        if self.text_at == 0 {
            return;
        }
        sceKernelDcacheWritebackRange(self.text_vb.as_ptr() as *const c_void, (self.text_at * core::mem::size_of::<TextVertex>()) as u32);
        sceGuDisable(GuState::DepthTest);
        sceGuDisable(GuState::CullFace);
        sceGuEnable(GuState::Blend);
        sceGuEnable(GuState::Texture2D);
        sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgba);
        sceGuTexFilter(TextureFilter::Nearest, TextureFilter::Nearest);
        sceGuTexWrap(GuTexWrapMode::Clamp, GuTexWrapMode::Clamp);
        sceGuTexMapMode(TextureMapMode::TextureCoords, 0, 0);
        sceGuTexScale(1.0, 1.0);
        sceGuTexOffset(0.0, 0.0);
        sceGuClutMode(ClutPixelFormat::Psm8888, 0, 0xff, 0);
        sceGuClutLoad(2, ptr::addr_of!(INK.0) as *const c_void);
        sceGuTexMode(TexturePixelFormat::PsmT8, 0, 0, 0);
        sceGuTexImage(MipmapLevel::None, text::ATLAS_W as i32, text::ATLAS_H as i32, text::ATLAS_W as i32, ptr::addr_of!(LETTERS.0) as *const c_void);
        sceGuDrawArray(GuPrimitive::Sprites, vtype_text(), self.text_at as i32, ptr::null(), self.text_vb.as_ptr() as *const c_void);
        sceGuDisable(GuState::Blend);
        self.stats.draws += 1;
        self.text_at = 0;
    }

    /// Chooses the frame's draws. The GE may still be drawing the frame before.
    pub unsafe fn choose(&mut self, flight: &Flight, streamer: &mut Streamer) {
        self.stats = Stats::default();
        let t0 = sceKernelGetSystemTimeLow();
        let (eye, look, fov) = flight.eye();
        let vp = mat::mul(&mat::perspective(fov, 480.0 / 272.0, 3.0, 12000.0), &mat::view(eye, look, 0.0));
        let planes = mat::planes(&vp);
        streamer.retire(&self.cells, eye, look, flight.show.near);
        let mut lists = core::mem::take(&mut self.lists);
        let tables = view::Tables { city: &self.city, regions: &self.regions, blocks: &self.blocks, cells: &self.cells, batches: &self.batches, spans: &self.spans, landmarks: &self.landmarks };
        let show = view::Reach { split: true, ..flight.show };
        let counts = view::select(&tables, &planes, eye, &show, &|cell| streamer.ready(cell), &mut lists);
        self.lists = lists;
        self.stats.places = counts.places;
        self.stats.turned = counts.turned;
        self.chosen = (counts.mid_from, 0);
        self.stats.select = sceKernelGetSystemTimeLow().wrapping_sub(t0);
    }

    /// Writes and starts the list of the frame chosen. No list is in flight when this is called.
    pub unsafe fn draw(&mut self, flight: &Flight, streamer: &mut Streamer) {
        let t1 = sceKernelGetSystemTimeLow();
        let (eye, look, fov) = flight.eye();
        let aspect = 480.0 / 272.0;
        streamer.refill(&self.city, &self.cells, eye, look, flight.show.near);
        let mid_from = self.chosen.0;

        let light = sky::light(flight.hour, DAY);
        let by_sector = sky::by_sector(&light);
        // (the texture stage doubles what it is given)
        let lit = self.option & 4 == 0;
        let lights: [u32; SECTORS + 1] = core::array::from_fn(|k| if lit { abgr(by_sector[k].map(|v| v * 0.5)) } else { 0xff80_8080 });
        // What stands in shadow keeps the sky's light, of what the sun and the sky give together.
        let (open, top) = (by_sector[SECTORS], light.night);
        let hour = Hour { step: (light.night * STEPS + 0.5) as u32, shade: [0, 1, 2].map(|k| (clamp(lerp(light.sky[k], 1.0, top) / max(open[k], 0.01), 0.0, 1.0) * 255.0) as u32) };
        if light.night < 0.97 {
            shade::aim(light.dir);
        }
        let mut budget = MIX_BUDGET;

        sceGuStart(GuContextType::Direct, ptr::addr_of_mut!(LIST.0) as *mut c_void);
        let haze = abgr(light.horizon);
        sceGuClearColor(haze);
        sceGuClearDepth(0);
        sceGuClear(ClearBuffer::COLOR_BUFFER_BIT | ClearBuffer::DEPTH_BUFFER_BIT);
        // The eye is the origin: a place's matrix carries where it is from there.
        sceGuSetMatrix(MatrixMode::View, &fmatrix(&mat::view(V3::ZERO, look, 0.0)));
        // The mid level starts where the nearest cell without its near level is.
        let near_far = flight.show.near + self.city.block / self.city.cells as f32 * 1.5 + 60.0;
        let far_near = clamp(min(mid_from, flight.show.mid) * 0.8, 6.0, 600.0);
        self.split = (far_near, near_far);
        let far_proj = fmatrix(&mat::perspective_gl(fov, aspect, far_near, 12000.0));
        let near_proj = fmatrix(&mat::perspective_gl(fov, aspect, 3.0, near_far));
        sceGuSetMatrix(MatrixMode::Projection, &far_proj);
        sceGuDepthRange(DEPTH_SPLIT, 0);
        self.sky(&light);

        sceGuEnable(GuState::DepthTest);
        if self.option & 1 == 0 {
            sceGuEnable(GuState::CullFace);
        }
        sceGuFrontFace(if self.option & 2 == 0 { FrontFaceDirection::CounterClockwise } else { FrontFaceDirection::Clockwise });
        if self.option & 8 == 0 {
            sceGuEnable(GuState::Fog);
            sceGuFog(120.0, 9000.0, haze);
        }
        if self.option & 16 == 0 {
            sceGuEnable(GuState::Lighting);
        }
        if self.option & 32 == 0 {
            sceGuEnable(GuState::Texture2D);
        }
        if self.option & 128 != 0 {
            sceGuDisable(GuState::ClipPlanes);
        } else {
            sceGuEnable(GuState::ClipPlanes);
        }
        sceGuEnable(GuState::Fragment2X);
        sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgb);
        if self.option & 64 == 0 {
            sceGuTexFilter(TextureFilter::LinearMipmapNearest, TextureFilter::Linear);
        } else {
            sceGuTexFilter(TextureFilter::NearestMipmapNearest, TextureFilter::Nearest);
        }
        sceGuTexLevelMode(TextureLevelMode::Auto, 0.0);

        for near in [true, false] {
            if near {
                sceGuSetMatrix(MatrixMode::Projection, &near_proj);
                sceGuDepthRange(65535, DEPTH_SPLIT);
            } else {
                sceGuSetMatrix(MatrixMode::Projection, &far_proj);
                sceGuDepthRange(DEPTH_SPLIT, 0);
            }
            // The ground and the roofs: the picture from above, placed by the texture matrix.
            sceGuSendCommandi(GeCommand::VertexType, vtype_top().bits());
            sceGuTexMapMode(TextureMapMode::TextureMatrix, 0, 0);
            sceGuTexProjMapMode(TextureProjectionMapMode::Position);
            sceGuTexWrap(GuTexWrapMode::Clamp, GuTexWrapMode::Clamp);
            self.pass(kind::TOP as usize, near, eye, streamer, &lights, &hour, &mut budget);
            // Walls: the facade pictures, repeated upwards.
            let facade = self.blocks.len();
            let p = self.pictures[facade];
            let data = self.htex.as_ptr().add(p.offset as usize);
            let out = self.mixed.as_mut_ptr().add(facade * 256);
            mix(data as *const u32, out, &mut self.held[facade], &hour, false, &mut budget);
            bind(out, data.add(2048), p.width as u32, p.height as u32, p.levels);
            sceGuSendCommandi(GeCommand::VertexType, vtype_wall().bits());
            sceGuTexMapMode(TextureMapMode::TextureCoords, 0, 0);
            sceGuTexWrap(GuTexWrapMode::Clamp, GuTexWrapMode::Repeat);
            sceGuTexScale(1.0, pack::FACADE_V);
            sceGuTexOffset(0.0, -pack::FACADE_V);
            self.pass(kind::WALL as usize, near, eye, streamer, &lights, &hour, &mut budget);
            // What is painted: one white texel under the vertex's colour.
            sceGuSendCommandi(GeCommand::VertexType, vtype_top().bits());
            sceGuTexMode(TexturePixelFormat::Psm5650, 0, 0, 0);
            sceGuTexImage(MipmapLevel::None, 8, 8, 8, ptr::addr_of!(WHITE.0) as *const c_void);
            sceGuTexScale(1.0, 1.0);
            sceGuTexOffset(0.0, 0.0);
            self.pass(kind::SOLID as usize, near, eye, streamer, &lights, &hour, &mut budget);
            // The landmarks' members, seen from both sides.
            sceGuDisable(GuState::CullFace);
            self.pass(kind::OPEN as usize, near, eye, streamer, &lights, &hour, &mut budget);
            if self.option & 1 == 0 {
                sceGuEnable(GuState::CullFace);
            }
        }
        self.stats.mixes = MIX_BUDGET - budget;
        sceGuDisable(GuState::Lighting);
        sceGuDisable(GuState::Fog);
        sceGuDisable(GuState::Fragment2X);
        self.letters();
        self.stats.words = (sceGuFinish() / 4) as u32;
        self.stats.list = sceKernelGetSystemTimeLow().wrapping_sub(t1);
    }
}
