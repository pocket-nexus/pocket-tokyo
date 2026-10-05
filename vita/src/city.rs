//! The city on the GPU: every draw of the pack in one block of GPU-mapped
//! memory, the pictures in video memory, and the per-frame choice of what to
//! draw.
//!
//! A frame walks the regions. One that lies far away draws its far level. In
//! a closer one each block in view draws its mid level, whole; a block the
//! eye is near draws those of its cells that are near at the near level, and
//! the rest of itself as runs of cells out of the same mid batches. The draws
//! are grouped by program, and of a batch of walls ordered by sector only
//! the arc that can face the eye is drawn.

use pocket_vita_gxm::mem::{Arena, Block as Memory, Kind};
use pocket_vita_gxm::program::{S16N, S8N, U16N, U8N};
use pocket_vita_gxm::texture::{Format, Texture, Uploader, Wrap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use tokyo_pack::{self as pack, kind, Batch, Block, Cell, City, GroundLevel, Region, TexHeader, LODS};
use tokyo_sim::math::*;
use tokyo_sim::view::{self, Item};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Cull, Gpu, Param, Program, Stream, Uniforms};
use tokyo_sim::mat::{self, Mat4};

const TOP_V: &str = include_str!("../shaders/top_v.cg");
const TOP_F: &str = include_str!("../shaders/top_f.cg");
const WALL_V: &str = include_str!("../shaders/wall_v.cg");
const WALL_F: &str = include_str!("../shaders/wall_f.cg");
const SOLID_V: &str = include_str!("../shaders/solid_v.cg");
const SOLID_F: &str = include_str!("../shaders/solid_f.cg");
const FRAME: &str = include_str!("../shaders/frame.cg");

/// Light reaches the fragments as a colour, which holds 0..1: it is carried divided by this.
pub const LIGHT_SCALE: f32 = 2.0;
/// Metres under the shadow height over which the sun's light goes out.
const SHADOW_SOFT: f32 = 2.5;

/// A pack on storage: the section table, and reads of what a section holds.
pub struct PackFile {
    file: File,
    index: pack::Index,
    pub path: String,
}

impl PackFile {
    pub fn open(path: &str) -> Result<PackFile, String> {
        let mut file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head).map_err(|e| format!("{path}: {e}"))?;
        let count = pack::Index::count(&head)?;
        let mut table = vec![0u8; pack::Index::head_bytes(count)];
        table[..16].copy_from_slice(&head);
        file.read_exact(&mut table[16..]).map_err(|e| format!("{path}: {e}"))?;
        Ok(PackFile { file, index: pack::Index::parse(&table)?, path: path.to_string() })
    }

    pub fn size(&self, tag: u32) -> Result<usize, String> {
        Ok(self.index.range(tag)?.1)
    }

    /// `len` bytes of a section from `at`, into `to`.
    pub unsafe fn read_into(&mut self, tag: u32, at: usize, to: *mut u8, len: usize) -> Result<(), String> {
        let (offset, size) = self.index.range(tag)?;
        if at + len > size {
            return Err("read past the end of a pack section".into());
        }
        self.file.seek(SeekFrom::Start((offset + at) as u64)).map_err(|e| e.to_string())?;
        // In pieces: one request of tens of megabytes holds the card for seconds.
        let mut done = 0;
        while done < len {
            let n = (len - done).min(1 << 20);
            self.file.read_exact(core::slice::from_raw_parts_mut(to.add(done), n)).map_err(|e| format!("{}: {e}", self.path))?;
            done += n;
        }
        Ok(())
    }

    pub fn read(&mut self, tag: u32) -> Result<Vec<u8>, String> {
        let n = self.size(tag)?;
        let mut bytes = vec![0u8; n];
        unsafe { self.read_into(tag, 0, bytes.as_mut_ptr(), n)? };
        Ok(bytes)
    }

    pub fn read_range(&mut self, tag: u32, at: usize, len: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; len];
        unsafe { self.read_into(tag, at, bytes.as_mut_ptr(), len)? };
        Ok(bytes)
    }
}

/// One of the three scene programs and where its uniforms go.
struct Pass {
    prog: Program,
    mvp: Param,
    map: Param,
    frame: Param,
    look: Param,
    /// Texture units of the program's pictures, of the shadow heights and of the lamp light.
    units: [u32; 2],
    shadow: u32,
    lamp: u32,
}

impl Pass {
    unsafe fn new(prog: Program, pictures: [&str; 2]) -> Pass {
        let units = pictures.map(|name| if name.is_empty() { 0 } else { prog.fs.sampler_index(name).unwrap_or(0) });
        Pass { mvp: prog.vs.param("uMvp"), map: prog.vs.param("uMap"), frame: prog.vs.param("uFrame"), look: prog.fs.param("uLook"), units, shadow: prog.fs.sampler_index("uShadow").unwrap_or(0), lamp: prog.fs.sampler_index("uLamp").unwrap_or(0), prog }
    }

    #[inline]
    unsafe fn uniforms(&self, ctx: *mut g::SceGxmContext, mvp: &Mat4, map: &[f32; 4], frame: &[f32; 16], look: &[f32; 8]) {
        let v = Uniforms::vertex(ctx);
        v.set(self.mvp, mvp);
        v.set(self.map, map);
        v.set(self.frame, frame);
        Uniforms::fragment(ctx).set(self.look, look);
    }
}

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub draws: u32,
    /// Triangles by program: tops, walls, solids.
    pub tris: [u32; 3],
    /// Places drawn at each level of detail: cells near, blocks mid, regions far.
    pub places: [u32; LODS],
    /// Wall triangles left out because they face away.
    pub turned: u32,
}

/// What a frame draws and at which distances the levels of detail hand over.
#[derive(Clone, Copy)]
pub struct Show {
    pub top: bool,
    pub wall: bool,
    pub solid: bool,
    pub near: f32,
    pub mid: f32,
    /// Leave out the walls that face away from the eye.
    pub sectors: bool,
    /// Draws each draw is cut into (1: as they are).
    pub chop: u32,
}

pub struct CityGpu {
    pub city: City,
    regions: Vec<Region>,
    blocks: Vec<Block>,
    cells: Vec<Cell>,
    batches: Vec<Batch>,
    spans: Vec<u32>,
    _geometry: Memory,
    vtop: *const u8,
    vwal: *const u8,
    vsol: *const u8,
    idx: *const u16,
    block_ground: Vec<Option<Texture>>,
    region_ground: Vec<Option<Texture>>,
    facade: Texture,
    night: Texture,
    /// Lamp light on the ground, over the height grid.
    lamp: g::SceGxmTexture,
    /// The top of whatever stands in each cell of the height grid.
    pub heights: Vec<u16>,
    /// The places of the tour: eye and target.
    pub tour: Vec<[f32; 6]>,
    /// The traffic's lanes and their points.
    pub lanes: (Vec<tokyo_sim::traffic::Lane>, Vec<[f32; 3]>),
    top: Pass,
    top_night: Pass,
    wall: Pass,
    wall_night: Pass,
    solid: Pass,
    /// This frame's draws, by program.
    lists: [Vec<Item>; 3],
    pub geometry_bytes: usize,
    pub texture_bytes: usize,
}

/// What every scene program is compiled with.
pub fn scene_defines(city: &City) -> String {
    // (a wall reads the height grid three metres out from itself, in its vertices' own units)
    format!(
        "#define FACADE_V {:.1}\n#define WALL_GAIN {:.5}\n#define WALL_OUT {:.6}\n#define LIGHT_SCALE {LIGHT_SCALE:.1}\n#define SHADOW_K {:.2}\n#define Y0 {:.3}\n#define Y_SPAN {:.3}\n{FRAME}",
        pack::FACADE_V,
        1.0 / 0.7354,
        3.0 / (city.block + 2.0 * city.margin),
        city.y_span / SHADOW_SOFT,
        city.y0,
        city.y_span
    )
}

impl CityGpu {
    /// # Safety
    /// GXM is initialized; `vram` outlives the city.
    pub unsafe fn load(p: &mut PackFile, gpu: &mut Gpu, vram: &mut Arena, msaa: u32, ground_top: u32, mut progress: impl FnMut(&str)) -> Result<CityGpu, String> {
        let city: City = pack::read(&p.read(pack::CITY)?, 0).ok_or("city record")?;
        let regions: Vec<Region> = pack::table(&p.read(pack::REGN)?);
        let blocks: Vec<Block> = pack::table(&p.read(pack::BLCK)?);
        let cells: Vec<Cell> = pack::table(&p.read(pack::CELL)?);
        let batches: Vec<Batch> = pack::table(&p.read(pack::BTCH)?);
        let spans: Vec<u32> = pack::table(&p.read(pack::SPAN)?);
        let levels: Vec<GroundLevel> = pack::table(&p.read(pack::GLVL)?);

        progress("Preparing programs");
        let defines = scene_defines(&city);
        let opaque = [Blend::Opaque, Blend::Alpha];
        let top_attrs = [("aPosition", 0u16, U16N, 3u8), ("aAo", 6, U8N, 2), ("aNormal", 8, S8N, 3)];
        let top = Pass::new(gpu.program("top", &defines, TOP_V, TOP_F, &[Stream { stride: 12, instanced: false, attrs: &top_attrs }], msaa, opaque)?, ["uGround", ""]);
        let top_night = Pass::new(gpu.program("top_night", &format!("{defines}#define NIGHT\n"), TOP_V, TOP_F, &[Stream { stride: 12, instanced: false, attrs: &top_attrs }], msaa, opaque)?, ["uGround", ""]);
        let wall_attrs = [("aPosition", 0u16, U16N, 3u8), ("aAo", 6, U8N, 2), ("aNormal", 8, S8N, 3), ("aLate", 11, U8N, 1), ("aColor", 12, U8N, 4), ("aUv", 16, S16N, 2)];
        let wall = Pass::new(gpu.program("wall", &defines, WALL_V, WALL_F, &[Stream { stride: 20, instanced: false, attrs: &wall_attrs }], msaa, opaque)?, ["uFacade", ""]);
        let wall_night = Pass::new(gpu.program("wall_night", &format!("{defines}#define NIGHT\n"), WALL_V, WALL_F, &[Stream { stride: 20, instanced: false, attrs: &wall_attrs }], msaa, opaque)?, ["uFacade", "uNight"]);
        let solid_attrs = [("aPosition", 0u16, U16N, 3u8), ("aAo", 6, U8N, 2), ("aNormal", 8, S8N, 3), ("aColor", 12, U8N, 4)];
        let solid = Pass::new(gpu.program("solid", &defines, SOLID_V, SOLID_F, &[Stream { stride: 16, instanced: false, attrs: &solid_attrs }], msaa, opaque)?, ["", ""]);

        progress("Reading the city's geometry");
        let sizes = [p.size(pack::VTOP)?, p.size(pack::VWAL)?, p.size(pack::VSOL)?, p.size(pack::IDX0)?];
        let pad = |n: usize| (n + 63) & !63;
        let total: usize = sizes.iter().map(|&n| pad(n)).sum();
        let mut block = Memory::with_access(Kind::Main, total, false)?;
        let mut at = [core::ptr::null_mut::<u8>(); 4];
        for (k, tag) in [pack::VTOP, pack::VWAL, pack::VSOL, pack::IDX0].into_iter().enumerate() {
            at[k] = block.alloc(pad(sizes[k]), 64).ok_or("geometry block")?;
            // A few megabytes at a time, with the loading screen in between.
            let mut done = 0;
            while done < sizes[k] {
                let n = (sizes[k] - done).min(4 << 20);
                p.read_into(tag, done, at[k].add(done), n)?;
                done += n;
                progress(&format!("Reading the city's geometry: {:.0} MB", (sizes[..k].iter().sum::<usize>() + done) as f32 / 1e6));
            }
        }

        progress("Uploading the facades");
        let mut up = Uploader::new(8 * 1024 * 1024)?;
        let mut texture_bytes = 0usize;
        let mut picture = |p: &mut PackFile, up: &mut Uploader, vram: &mut Arena, tag: u32, format: Format| -> Result<Texture, String> {
            let bytes = p.read(tag)?;
            let head: TexHeader = pack::read(&bytes, 0).ok_or("texture header")?;
            let t = up.texture(vram, format, head.width, head.height, head.mips, &bytes[core::mem::size_of::<TexHeader>()..])?;
            up.flush();
            texture_bytes += bytes.len();
            Ok(t)
        };
        let mut facade = picture(p, &mut up, vram, pack::FACD, Format::Bc3)?;
        let mut night = picture(p, &mut up, vram, pack::FACN, Format::Bc1)?;
        for t in [&mut facade, &mut night] {
            t.set_wrap(Wrap::Clamp, Wrap::Repeat);
            t.set_filter(true, true);
        }

        // A picture from above: the largest level this run allows, and every smaller one.
        let mut chain = |p: &mut PackFile, up: &mut Uploader, vram: &mut Arena, first: u32, count: u32, largest: u32| -> Result<Option<Texture>, String> {
            if count == 0 {
                return Ok(None);
            }
            let all = &levels[first as usize..(first + count) as usize];
            let from = all.iter().position(|l| l.width <= largest).unwrap_or(all.len() - 1);
            let chosen = &all[from..];
            let bytes = p.read_range(pack::GTEX, chosen[0].offset as usize, chosen.iter().map(|l| l.size as usize).sum())?;
            let mut tex = up.texture(vram, Format::Bc1, chosen[0].width, chosen[0].width, chosen.len() as u32, &bytes)?;
            up.flush();
            tex.set_wrap(Wrap::Clamp, Wrap::Clamp);
            tex.set_filter(true, true);
            texture_bytes += bytes.len();
            Ok(Some(tex))
        };
        let mut block_ground = Vec::with_capacity(blocks.len());
        for (i, b) in blocks.iter().enumerate() {
            block_ground.push(chain(p, &mut up, vram, b.ground_first, b.ground_levels, ground_top)?);
            progress(&format!("Uploading the ground: {} of {} blocks", i + 1, blocks.len()));
        }
        let mut region_ground = Vec::with_capacity(regions.len());
        for r in &regions {
            region_ground.push(chain(p, &mut up, vram, r.ground_first, r.ground_levels, 4096)?);
        }
        up.free();
        let heights: Vec<u16> = pack::table(&p.read(pack::HMAP)?);
        let tour: Vec<[f32; 6]> = pack::table(&p.read(pack::TOUR)?);
        let lanes = (pack::table(&p.read(pack::LANE)?), pack::table(&p.read(pack::LPTS)?));
        // Lamp light: 16-bit texels over the height grid, as they are in the pack.
        let lamp_bytes = p.read(pack::LAMP)?;
        if lamp_bytes.len() != (city.grid_w * city.grid_h * 2) as usize {
            return Err("the lamp light does not match the height grid".into());
        }
        let lamp_at = vram.alloc(lamp_bytes.len(), 512)?;
        core::ptr::copy_nonoverlapping(lamp_bytes.as_ptr(), lamp_at, lamp_bytes.len());
        let mut lamp: g::SceGxmTexture = core::mem::zeroed();
        if g::sceGxmTextureInitLinear(&mut lamp, lamp_at.cast(), g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U5U6U5_RGB, city.grid_w, city.grid_h, 1) < 0 {
            return Err("lamp light texture".into());
        }
        g::sceGxmTextureSetMinFilter(&mut lamp, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        g::sceGxmTextureSetMagFilter(&mut lamp, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        texture_bytes += lamp_bytes.len();
        Ok(CityGpu {
            city,
            regions,
            blocks,
            cells,
            batches,
            spans,
            vtop: at[0],
            vwal: at[1],
            vsol: at[2],
            idx: at[3].cast(),
            geometry_bytes: block.size(),
            _geometry: block,
            block_ground,
            region_ground,
            facade,
            night,
            lamp,
            heights,
            tour,
            lanes,
            top,
            top_night,
            wall,
            wall_night,
            solid,
            lists: [Vec::with_capacity(512), Vec::with_capacity(512), Vec::with_capacity(512)],
            texture_bytes,
        })
    }

    /// The top of whatever stands at a point, metres.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let c = &self.city;
        let i = (((x - c.grid_x0) / c.grid_step) as i32).clamp(0, c.grid_w as i32 - 1) as usize;
        let j = (((z - c.grid_z0) / c.grid_step) as i32).clamp(0, c.grid_h as i32 - 1) as usize;
        c.y0 + self.heights[j * c.grid_w as usize + i] as f32 * c.height_step
    }

    /// Draws the city from `eye`.
    ///
    /// # Safety
    /// Inside a scene on `ctx`.
    pub unsafe fn draw(&mut self, ctx: *mut g::SceGxmContext, vp: &Mat4, eye: V3, frame: &[f32; 16], look: &[f32; 8], shadow: &g::SceGxmTexture, show: &Show) -> Stats {
        let planes = mat::planes(vp);
        let mut stats = Stats::default();
        let mut lists = core::mem::take(&mut self.lists);
        for l in &mut lists {
            l.clear();
        }
        let tables = view::Tables { city: &self.city, regions: &self.regions, blocks: &self.blocks, cells: &self.cells, batches: &self.batches, spans: &self.spans, cards: &[] };
        let counts = view::select(&tables, &planes, eye, &view::Reach { near: show.near, mid: show.mid, sectors: show.sectors, split: false }, &|_| true, &mut lists);
        stats.places = counts.places;
        stats.turned = counts.turned;
        let c = &self.city;
        let night = frame[7] > 0.01;
        let grid = (c.grid_w as f32 * c.grid_step, c.grid_h as f32 * c.grid_step);
        for k in [kind::TOP, kind::WALL, kind::SOLID] {
            let list = &lists[k as usize];
            let on = match k {
                kind::TOP => show.top,
                kind::WALL => show.wall,
                _ => show.solid,
            };
            if list.is_empty() || !on {
                continue;
            }
            let (pass, vertices, stride) = match k {
                kind::TOP => (if night { &self.top_night } else { &self.top }, self.vtop, 12),
                kind::WALL => (if night { &self.wall_night } else { &self.wall }, self.vwal, 20),
                _ => (&self.solid, self.vsol, 16),
            };
            pass.prog.bind(ctx, false);
            gpu::state_opaque(ctx, if k == kind::SOLID { Cull::None } else { Cull::Cw });
            g::sceGxmSetFragmentTexture(ctx, pass.shadow, shadow);
            if k == kind::TOP && night {
                g::sceGxmSetFragmentTexture(ctx, pass.lamp, &self.lamp);
            }
            if k == kind::WALL {
                g::sceGxmSetFragmentTexture(ctx, pass.units[0], &self.facade.gxm);
                if night {
                    g::sceGxmSetFragmentTexture(ctx, pass.units[1], &self.night.gxm);
                }
            }
            let mut last = (u32::MAX, false);
            let mut mvp = [0.0f32; 16];
            let mut map = [0.0f32; 4];
            for item in list {
                if (item.place, item.region) != last {
                    last = (item.place, item.region);
                    let (origin, span) = view::frame_of(c, item.place, item.region, k == kind::TOP);
                    mvp = mat::with_bounds(vp, origin, span);
                    map = [span[0] / grid.0, span[2] / grid.1, (origin[0] - c.grid_x0) / grid.0, (origin[2] - c.grid_z0) / grid.1];
                    if k == kind::TOP {
                        let pictures = if item.region { &self.region_ground } else { &self.block_ground };
                        if let Some(tex) = &pictures[item.place as usize] {
                            g::sceGxmSetFragmentTexture(ctx, pass.units[0], &tex.gxm);
                        }
                    }
                }
                let b = &self.batches[item.batch as usize];
                // (`chop` cuts every draw into that many, to measure what a draw costs by itself)
                let step = if show.chop > 1 { ((item.to - item.from) / 3).div_ceil(show.chop).max(1) * 3 } else { item.to - item.from };
                let mut from = item.from;
                while from < item.to {
                    let to = (from + step).min(item.to);
                    pass.uniforms(ctx, &mvp, &map, frame, look);
                    gpu::draw(ctx, vertices.add(b.vtx_first as usize * stride), self.idx.add((b.idx_first + from) as usize), to - from);
                    stats.draws += 1;
                    from = to;
                }
                stats.tris[k as usize] += (item.to - item.from) / 3;
            }
        }
        self.lists = lists;
        stats
    }
}
