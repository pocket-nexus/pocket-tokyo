//! The city on the GPU: the pack's vertices and indices in four buffers, the
//! pictures as ETC2 textures, and the per-frame choice of what to draw.
//!
//! A frame walks the regions (`tokyo_sim::view::select`). One that lies far
//! away draws its far level. In a closer one each block in view draws its mid
//! level; a block the eye is near draws those of its cells that are near at
//! the near level, and the rest of itself as runs of cells out of the same mid
//! batches. Of a batch of walls ordered by sector only the arc that can face
//! the eye is drawn. A landmark is a model of its own, at the level of detail
//! of its distance.
//!
//! A batch's indices are 16 bits and count from the batch's first vertex, and
//! OpenGL ES 3.0 has no draw call that adds a base to an index: each batch has
//! a vertex array of its own, which starts at that vertex. (Indices of 32 bits
//! into one array were tried: this driver walks them on the CPU at every
//! draw, 5 to 30 ns an index.)

use core::ffi::c_void;
use tokyo_pack::{self as pack, flag, kind, Batch, Block, Cell, City, GroundLevel, Landmark, Region, TexHeader, LODS};
use tokyo_sim::mat::{self, Mat4};
use tokyo_sim::math::*;
use tokyo_sim::view::{self, Item, Reach};

use crate::gl;

const FRAME: &str = include_str!("../../shaders/frame.glsl");
const TOP_V: &str = include_str!("../../shaders/top.vert");
const TOP_F: &str = include_str!("../../shaders/top.frag");
const WALL_V: &str = include_str!("../../shaders/wall.vert");
const WALL_F: &str = include_str!("../../shaders/wall.frag");
const SOLID_V: &str = include_str!("../../shaders/solid.vert");
pub const SOLID_F: &str = include_str!("../../shaders/solid.frag");

/// Metres under the shadow height over which the sun's light goes out.
const SHADOW_SOFT: f32 = 2.5;
/// Bytes read from the pack and handed to the GPU between two frames of the loading screen.
const CHUNK: usize = 2 << 20;

extern "C" {
    fn pread64(fd: i32, to: *mut c_void, count: usize, offset: i64) -> isize;
}

/// A pack on storage (a file of its own, or a stored entry of the package): the section table, and reads
/// of what a section holds.
pub struct PackFile {
    fd: i32,
    base: i64,
    index: pack::Index,
}

impl PackFile {
    pub fn open(fd: i32, base: i64, length: i64) -> Result<PackFile, String> {
        let mut head = [0u8; 16];
        Self::at(fd, base, &mut head)?;
        let count = pack::Index::count(&head)?;
        let mut table = vec![0u8; pack::Index::head_bytes(count)];
        Self::at(fd, base, &mut table)?;
        let index = pack::Index::parse(&table)?;
        if index.sections.iter().any(|s| (s.1 + s.2) as i64 > length) {
            return Err("the pack is shorter than its table says".into());
        }
        Ok(PackFile { fd, base, index })
    }

    fn at(fd: i32, offset: i64, to: &mut [u8]) -> Result<(), String> {
        let mut done = 0;
        while done < to.len() {
            let n = unsafe { pread64(fd, to[done..].as_mut_ptr().cast(), to.len() - done, offset + done as i64) };
            if n <= 0 {
                return Err("the pack could not be read".into());
            }
            done += n as usize;
        }
        Ok(())
    }

    pub fn size(&self, tag: u32) -> Result<usize, String> {
        Ok(self.index.range(tag)?.1)
    }

    /// Bytes of a section from `at`, into `to`.
    pub fn read_into(&self, tag: u32, at: usize, to: &mut [u8]) -> Result<(), String> {
        let (offset, size) = self.index.range(tag)?;
        if at + to.len() > size {
            return Err("read past the end of a pack section".into());
        }
        Self::at(self.fd, self.base + (offset + at) as i64, to)
    }

    pub fn read(&self, tag: u32) -> Result<Vec<u8>, String> {
        self.read_range(tag, 0, self.size(tag)?)
    }

    pub fn read_range(&self, tag: u32, at: usize, len: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; len];
        self.read_into(tag, at, &mut bytes)?;
        Ok(bytes)
    }
}

/// One of the scene programs and where its uniforms go.
#[derive(Clone, Copy, Default)]
pub struct Pass {
    pub program: u32,
    mvp: i32,
    map: i32,
    frame: i32,
    light: i32,
}

/// The fragment stage's light: the ambient colour, the sun's, and the haze with how far the lamps are on.
pub type Light = [f32; 12];

impl Pass {
    /// # Safety
    /// A current context.
    pub unsafe fn new(name: &str, defines: &str, vs: &str, fs: &str, attributes: &[&str]) -> Result<Pass, String> {
        let program = gl::program(name, "", &format!("{defines}{FRAME}{vs}"), &format!("{defines}{fs}"), attributes)?;
        gl::samplers(program, &[("uGround", 0), ("uFacade", 0), ("uShadow", 1), ("uLamp", 2), ("uNight", 2)]);
        Ok(Pass { program, mvp: gl::uniform(program, "uMvp"), map: gl::uniform(program, "uMap"), frame: gl::uniform(program, "uFrame"), light: gl::uniform(program, "uLight") })
    }

    /// Binds the program with the frame's values.
    pub unsafe fn begin(&self, frame: &[f32; 24], light: &Light) {
        gl::glUseProgram(self.program);
        gl::glUniform4fv(self.frame, 6, frame.as_ptr());
        gl::glUniform4fv(self.light, 3, light.as_ptr());
    }

    #[inline]
    pub unsafe fn place(&self, mvp: &Mat4, map: &[f32; 4]) {
        // (the matrices are stored by rows, as `M × v` reads them)
        gl::glUniformMatrix4fv(self.mvp, 1, 1, mvp.as_ptr());
        gl::glUniform4fv(self.map, 1, map.as_ptr());
    }
}

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub draws: u32,
    /// Triangles by kind of batch: tops, walls, solids, landmarks.
    pub tris: [u32; pack::KINDS],
    /// Places drawn at each level of detail: cells near, blocks mid, regions far.
    pub places: [u32; LODS],
    /// Wall triangles left out because they face away.
    pub turned: u32,
    /// Changes of place among the draws: each is a matrix and a picture the GPU is handed again.
    pub places_drawn: u32,
    /// Milliseconds of the render thread: choosing the draws, and the draw calls.
    pub ms: [f32; 2],
}

/// What a frame leaves out, for measurements.
#[derive(Clone, Copy)]
pub struct Show {
    pub top: bool,
    pub wall: bool,
    pub solid: bool,
    /// Draw in the order the pack lists places instead of nearest first.
    pub unordered: bool,
}

pub struct CityGpu {
    pub city: City,
    regions: Vec<Region>,
    blocks: Vec<Block>,
    cells: Vec<Cell>,
    batches: Vec<Batch>,
    spans: Vec<u32>,
    landmarks: Vec<Landmark>,
    /// A vertex array per batch: its kind's attributes from the batch's first vertex, and the buffer of indices.
    arrays: Vec<u32>,
    block_ground: Vec<u32>,
    region_ground: Vec<u32>,
    facade: u32,
    night: u32,
    /// Lamp light on the ground, over the height grid.
    lamp: u32,
    /// The top of whatever stands in each cell of the height grid.
    pub heights: Vec<u16>,
    /// The places of the tour: eye and target.
    pub tour: Vec<[f32; 6]>,
    /// The traffic's lanes and their points.
    pub lanes: (Vec<tokyo_sim::traffic::Lane>, Vec<[f32; 3]>),
    /// The ground's programs by day and by night, and the walls' by day, through the dusk and by night.
    top: [Pass; 2],
    wall: [Pass; 3],
    solid: Pass,
    lists: [Vec<Item>; pack::KINDS],
    order: Vec<(u64, u32)>,
    pub geometry_bytes: usize,
    pub texture_bytes: usize,
    pub name: String,
}

/// What every scene program is compiled with.
pub fn scene_defines(city: &City) -> String {
    // (a wall reads the height grid three metres out from itself, in its vertices' own units)
    format!(
        "#define FACADE_V {:.1}\n#define WALL_GAIN {:.5}\n#define WALL_OUT {:.6}\n#define SHADOW_K {:.2}\n#define Y0 {:.3}\n#define Y_SPAN {:.3}\n",
        pack::FACADE_V,
        1.0 / 0.7354,
        3.0 / (city.block + 2.0 * city.margin),
        city.y_span / SHADOW_SOFT,
        city.y0,
        city.y_span
    )
}

/// An ETC2 picture from its levels, largest first, as the pack stores them.
unsafe fn compressed(format: gl::Enum, mut w: u32, mut h: u32, levels: u32, bytes: &[u8], block: usize) -> Result<u32, String> {
    let mut id = 0;
    gl::glGenTextures(1, &mut id);
    gl::bind(0, id);
    let mut at = 0;
    for level in 0..levels {
        let size = (w.max(4) as usize / 4) * (h.max(4) as usize / 4) * block;
        if at + size > bytes.len() {
            return Err("a picture of the pack is short of its levels".into());
        }
        gl::glCompressedTexImage2D(gl::TEXTURE_2D, level as i32, format, w as i32, h as i32, 0, size as i32, bytes[at..].as_ptr().cast());
        at += size;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    // The pack stops its chains above one texel: the sampler stops there too.
    gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAX_LEVEL, levels as i32 - 1);
    // One level a fetch: the mean of two costs this GPU as much again.
    gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR_MIPMAP_NEAREST as i32);
    gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
    gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
    gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
    Ok(id)
}

/// The city while it is read: a step does a few megabytes or a picture, so that the loading screen is
/// drawn between two.
pub struct Loader {
    p: PackFile,
    city: Option<CityGpu>,
    levels: Vec<GroundLevel>,
    buffers: [u32; 4],
    sizes: [usize; 4],
    /// Which buffer is being filled and how far, then which picture is next.
    stage: usize,
    done: usize,
    chunk: Vec<u8>,
    /// The largest level of a ground picture this run takes.
    ground_top: u32,
}

impl Loader {
    pub fn new(fd: i32, base: i64, length: i64, ground_top: u32) -> Result<Loader, String> {
        Ok(Loader { p: PackFile::open(fd, base, length)?, city: None, levels: Vec::new(), buffers: [0; 4], sizes: [0; 4], stage: 0, done: 0, chunk: Vec::new(), ground_top })
    }

    /// One step. `Ok(Some(line))`: more to do, and what the loading screen says meanwhile.
    ///
    /// # Safety
    /// A current context, the same at every step.
    pub unsafe fn step(&mut self) -> Result<Option<String>, String> {
        let p = &self.p;
        if self.city.is_none() {
            let city: City = pack::read(&p.read(pack::CITY)?, 0).ok_or("city record")?;
            if city.flags & flag::ETC2 == 0 || city.flags & flag::LANDMARKS == 0 || city.flags & flag::STREAMED != 0 {
                return Err("the pack is not compiled for this device (profiles/redmi1s60.json)".into());
            }
            let meta = p.read(pack::META)?;
            let name = core::str::from_utf8(&meta).ok().and_then(|m| m.split("\"name\":\"").nth(1)).and_then(|m| m.split('"').next()).unwrap_or("Tokyo").to_string();
            let defines = scene_defines(&city);
            // By the hour: by day shadows and no lamps, by night lamps and no shadows (the moon's are faint).
            // Read together they cost a frame 2 ms, so only the walls have a set with both, for the dusk,
            // when their rooms come on under the last of the sun.
            let hours = [format!("{defines}#define SHADOWS\n"), format!("{defines}#define SHADOWS\n#define LAMPS\n"), format!("{defines}#define LAMPS\n")];
            let tops = ["aPosition", "aAo", "aNormal"];
            let walls = ["aPosition", "aAo", "aNormal", "aLate", "aColor", "aUv"];
            let solids = ["aPosition", "aAo", "aNormal", "aColor"];
            self.levels = pack::table(&p.read(pack::GLVL)?);
            self.sizes = [p.size(pack::VTOP)?, p.size(pack::VWAL)?, p.size(pack::VSOL)?, p.size(pack::IDX0)?];
            gl::glGenBuffers(4, self.buffers.as_mut_ptr());
            for k in 0..4 {
                let target = if k == 3 { gl::ELEMENT_ARRAY_BUFFER } else { gl::ARRAY_BUFFER };
                gl::glBindVertexArray(0);
                gl::glBindBuffer(target, self.buffers[k]);
                gl::glBufferData(target, self.sizes[k] as isize, core::ptr::null(), gl::STATIC_DRAW);
            }
            // Every attribute starts on a 4-byte boundary, and two may read the same bytes.
            let batches: Vec<Batch> = pack::table(&p.read(pack::BTCH)?);
            let mut arrays = vec![0u32; batches.len()];
            gl::glGenVertexArrays(arrays.len() as i32, arrays.as_mut_ptr());
            for (b, &array) in batches.iter().zip(&arrays) {
                let k = match b.kind {
                    kind::TOP => 0,
                    kind::WALL => 1,
                    _ => 2,
                };
                let stride = [12, 20, 16][k];
                let base = b.vtx_first as usize * stride as usize;
                let attribute = |index: u32, size: i32, kind: gl::Enum, offset: usize| {
                    gl::glEnableVertexAttribArray(index);
                    gl::glVertexAttribPointer(index, size, kind, 1, stride, (base + offset) as *const c_void);
                };
                gl::glBindVertexArray(array);
                gl::glBindBuffer(gl::ARRAY_BUFFER, self.buffers[k]);
                gl::glBindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.buffers[3]);
                attribute(0, 4, gl::UNSIGNED_SHORT, 0);
                attribute(1, 4, gl::UNSIGNED_BYTE, 4);
                attribute(2, 4, gl::BYTE, 8);
                match k {
                    1 => {
                        attribute(3, 4, gl::UNSIGNED_BYTE, 8);
                        attribute(4, 4, gl::UNSIGNED_BYTE, 12);
                        attribute(5, 2, gl::SHORT, 16);
                    }
                    2 => attribute(3, 4, gl::UNSIGNED_BYTE, 12),
                    _ => {}
                }
            }
            gl::glBindVertexArray(0);
            self.city = Some(CityGpu {
                city,
                regions: pack::table(&p.read(pack::REGN)?),
                blocks: pack::table(&p.read(pack::BLCK)?),
                cells: pack::table(&p.read(pack::CELL)?),
                batches,
                spans: pack::table(&p.read(pack::SPAN)?),
                landmarks: pack::table(&p.read(pack::LAND)?),
                arrays,
                block_ground: Vec::new(),
                region_ground: Vec::new(),
                facade: 0,
                night: 0,
                lamp: 0,
                heights: Vec::new(),
                tour: pack::table(&p.read(pack::TOUR)?),
                lanes: (pack::table(&p.read(pack::LANE)?), pack::table(&p.read(pack::LPTS)?)),
                top: [Pass::new("top (day)", &hours[0], TOP_V, TOP_F, &tops)?, Pass::new("top (night)", &hours[2], TOP_V, TOP_F, &tops)?],
                wall: [Pass::new("wall (day)", &hours[0], WALL_V, WALL_F, &walls)?, Pass::new("wall (dusk)", &hours[1], WALL_V, WALL_F, &walls)?, Pass::new("wall (night)", &hours[2], WALL_V, WALL_F, &walls)?],
                solid: Pass::new("solid", &defines, SOLID_V, SOLID_F, &solids)?,
                lists: core::array::from_fn(|_| Vec::with_capacity(512)),
                order: Vec::with_capacity(512),
                geometry_bytes: self.sizes.iter().sum(),
                texture_bytes: 0,
                name,
            });
            self.chunk = vec![0u8; CHUNK];
            return Ok(Some("Reading the city's geometry".into()));
        }
        let c = self.city.as_mut().unwrap();
        // ---- the vertices and the indices, a few megabytes a step
        if self.stage < 4 {
            let k = self.stage;
            let n = (self.sizes[k] - self.done).min(CHUNK);
            p.read_into([pack::VTOP, pack::VWAL, pack::VSOL, pack::IDX0][k], self.done, &mut self.chunk[..n])?;
            let target = if k == 3 { gl::ELEMENT_ARRAY_BUFFER } else { gl::ARRAY_BUFFER };
            gl::glBindVertexArray(0);
            gl::glBindBuffer(target, self.buffers[k]);
            gl::glBufferSubData(target, self.done as isize, n as isize, self.chunk.as_ptr().cast());
            self.done += n;
            let read = self.sizes[..k].iter().sum::<usize>() + self.done;
            if self.done >= self.sizes[k] {
                self.stage += 1;
                self.done = 0;
            }
            return Ok(Some(format!("Reading the city's geometry: {:.0} of {:.0} MB", read as f32 / 1e6, c.geometry_bytes as f32 / 1e6)));
        }
        // ---- the facades
        if self.stage == 4 {
            self.chunk = Vec::new();
            for (tag, format, block) in [(pack::FACD, gl::COMPRESSED_RGBA8_ETC2_EAC, 16), (pack::FACN, gl::COMPRESSED_RGB8_ETC2, 8)] {
                let bytes = p.read(tag)?;
                let head: TexHeader = pack::read(&bytes, 0).ok_or("texture header")?;
                let id = compressed(format, head.width, head.height, head.mips, &bytes[core::mem::size_of::<TexHeader>()..], block)?;
                // A wall repeats the picture upwards, storey by storey.
                gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::REPEAT as i32);
                c.texture_bytes += bytes.len();
                if tag == pack::FACD {
                    c.facade = id;
                } else {
                    c.night = id;
                }
            }
            self.stage += 1;
            return Ok(Some("Uploading the ground".into()));
        }
        // ---- a picture from above a step: the largest level this run allows, and every smaller one
        if self.stage == 5 {
            let (blocks, regions) = (c.blocks.len(), c.regions.len());
            let i = self.done;
            let (first, count, largest) = if i < blocks { (c.blocks[i].ground_first, c.blocks[i].ground_levels, self.ground_top) } else { (c.regions[i - blocks].ground_first, c.regions[i - blocks].ground_levels, 4096) };
            let id = if count == 0 {
                0
            } else {
                let all = &self.levels[first as usize..(first + count) as usize];
                let from = all.iter().position(|l| l.width <= largest).unwrap_or(all.len() - 1);
                let chosen = &all[from..];
                let bytes = p.read_range(pack::GTEX, chosen[0].offset as usize, chosen.iter().map(|l| l.size as usize).sum())?;
                c.texture_bytes += bytes.len();
                compressed(gl::COMPRESSED_RGB8_ETC2, chosen[0].width, chosen[0].width, chosen.len() as u32, &bytes, 8)?
            };
            if i < blocks {
                c.block_ground.push(id);
            } else {
                c.region_ground.push(id);
            }
            self.done += 1;
            if self.done == blocks + regions {
                self.stage += 1;
            }
            return Ok(Some(format!("Uploading the ground: {} of {} pictures", self.done, blocks + regions)));
        }
        // ---- the heights, and the lamps' light over the same grid
        if self.stage == 6 {
            c.heights = pack::table(&p.read(pack::HMAP)?);
            let lamp = p.read(pack::LAMP)?;
            let (w, h) = (c.city.grid_w as i32, c.city.grid_h as i32);
            if lamp.len() != (w * h * 2) as usize || c.heights.len() != (w * h) as usize {
                return Err("the lamp light does not match the height grid".into());
            }
            gl::glGenTextures(1, &mut c.lamp);
            gl::bind(0, c.lamp);
            gl::glPixelStorei(gl::UNPACK_ALIGNMENT, 2);
            gl::glTexImage2D(gl::TEXTURE_2D, 0, gl::RGB565 as i32, w, h, 0, gl::RGB, gl::UNSIGNED_SHORT_5_6_5, lamp.as_ptr().cast());
            // Seen from far away the grid is many texels to a pixel: without smaller levels each fetch
            // misses the texture cache.
            gl::glGenerateMipmap(gl::TEXTURE_2D);
            gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR_MIPMAP_NEAREST as i32);
            gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
            gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            gl::glTexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
            c.texture_bytes += lamp.len() * 4 / 3;
            self.stage += 1;
            return Ok(Some("Casting the first shadows".into()));
        }
        Ok(None)
    }

    pub fn finish(self) -> CityGpu {
        self.city.unwrap()
    }
}

impl CityGpu {
    /// Draws the city from `eye`. `cull`: the view and projection with depth from 0 to 1, which the
    /// frustum's planes are read from; `vp`: the same with OpenGL's depth. `lights`: the fragment stage's
    /// light for the ground and roofs, then for the walls.
    ///
    /// # Safety
    /// A current context with the frame's target bound.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn draw(&mut self, cull: &Mat4, vp: &Mat4, eye: V3, frame: &[f32; 24], lights: &[Light; 2], shadow: u32, reach: &Reach, show: &Show) -> Stats {
        let t0 = std::time::Instant::now();
        let planes = mat::planes(cull);
        let mut stats = Stats::default();
        let mut lists = core::mem::take(&mut self.lists);
        let mut order = core::mem::take(&mut self.order);
        let tables = view::Tables { city: &self.city, regions: &self.regions, blocks: &self.blocks, cells: &self.cells, batches: &self.batches, spans: &self.spans, landmarks: &self.landmarks };
        let counts = view::select(&tables, &planes, eye, reach, &|_| true, &mut lists);
        stats.places = counts.places;
        stats.turned = counts.turned;
        let c = &self.city;
        // (frame[1].w: how far the city's lights are on)
        let hour = if frame[7] <= 0.01 { 0 } else if frame[7] < crate::NIGHT { 1 } else { 2 };
        // (the ground's light, last number: how far the lamps' light lies on it)
        let pooled = lights[0][11] > 0.0;
        let grid = (c.grid_w as f32 * c.grid_step, c.grid_h as f32 * c.grid_step);
        gl::bind(1, shadow);
        // The walls, which stand before the ground and the roofs they hide, then what is painted, then the
        // ground and the roofs. Within a kind the places nearest the eye come first, and within a place its
        // near cells, nearest first, before its mid level: a fragment behind what is already drawn costs
        // a depth test and no more.
        let t1 = std::time::Instant::now();
        let per = c.region_blocks as usize;
        for k in [kind::WALL, kind::SOLID, kind::OPEN, kind::TOP] {
            let list = &lists[k as usize];
            let on = match k {
                kind::TOP => show.top,
                kind::WALL => show.wall,
                _ => show.solid,
            };
            if !on || list.is_empty() {
                continue;
            }
            order.clear();
            for (i, item) in list.iter().enumerate() {
                let (nx, side, ys) = if item.region {
                    let r = &self.regions[item.place as usize];
                    (c.blocks_x as usize / per, c.block * per as f32, (r.y_min, r.y_max))
                } else {
                    let b = &self.blocks[item.place as usize];
                    (c.blocks_x as usize, c.block, (b.y_min, b.y_max))
                };
                let (x0, z0) = (c.x0 + (item.place as usize % nx) as f32 * side, c.z0 + (item.place as usize / nx) as f32 * side);
                let place = mat::box_distance(eye, &[x0, ys.0, z0], &[x0 + side, ys.1, z0 + side]);
                let near = item.cell != u32::MAX;
                let within = if near {
                    let b = &self.batches[item.batch as usize];
                    mat::box_distance(eye, &b.min, &b.max)
                } else {
                    0.0
                };
                // (metres from the eye to the place; the place; near before mid; metres to the cell; the draw)
                let key = if show.unordered {
                    i as u64
                } else {
                    ((place as u64).min(0xffff) << 44) | ((item.region as u64) << 43) | ((item.place as u64 & 0x7ff) << 32) | ((!near as u64) << 31) | (((within * 4.0) as u64).min(0x7fff) << 16) | i as u64
                };
                order.push((key, i as u32));
            }
            order.sort_unstable();
            let pass = match k {
                kind::TOP => &self.top[pooled as usize],
                kind::WALL => &self.wall[hour],
                _ => &self.solid,
            };
            pass.begin(frame, &lights[(k == kind::WALL) as usize]);
            // (a landmark's members and what is painted are seen from both sides)
            if k == kind::SOLID || k == kind::OPEN {
                gl::glDisable(gl::CULL_FACE);
            } else {
                gl::glEnable(gl::CULL_FACE);
            }
            if k == kind::TOP && pooled {
                gl::bind(2, self.lamp);
            }
            if k == kind::WALL {
                if hour != 0 {
                    gl::bind(2, self.night);
                }
                gl::bind(0, self.facade);
            }
            gl::glActiveTexture(gl::TEXTURE0);
            let mut last = (u32::MAX, false);
            let mut array = u32::MAX;
            for &(_, i) in order.iter() {
                let item = &list[i as usize];
                if (item.place, item.region) != last {
                    last = (item.place, item.region);
                    let (origin, span) = view::frame_of(c, item.place, item.region, k == kind::TOP);
                    let mvp = mat::with_bounds(vp, origin, span);
                    pass.place(&mvp, &[span[0] / grid.0, span[2] / grid.1, (origin[0] - c.grid_x0) / grid.0, (origin[2] - c.grid_z0) / grid.1]);
                    if k == kind::TOP {
                        let pictures = if item.region { &self.region_ground } else { &self.block_ground };
                        gl::glBindTexture(gl::TEXTURE_2D, pictures[item.place as usize]);
                    }
                    stats.places_drawn += 1;
                }
                if item.batch != array {
                    array = item.batch;
                    gl::glBindVertexArray(self.arrays[array as usize]);
                }
                let b = &self.batches[item.batch as usize];
                gl::glDrawElements(gl::TRIANGLES, (item.to - item.from) as i32, gl::UNSIGNED_SHORT, ((b.idx_first + item.from) as usize * 2) as *const c_void);
                stats.draws += 1;
                stats.tris[k as usize] += (item.to - item.from) / 3;
            }
        }
        let t2 = std::time::Instant::now();
        stats.ms = [(t1 - t0).as_secs_f32() * 1000.0, (t2 - t1).as_secs_f32() * 1000.0];
        gl::glBindVertexArray(0);
        self.lists = lists;
        self.order = order;
        stats
    }
}

/// The top of whatever stands at a point, metres.
pub fn height(c: &City, heights: &[u16], x: f32, z: f32) -> f32 {
    let i = (((x - c.grid_x0) / c.grid_step) as i32).clamp(0, c.grid_w as i32 - 1) as usize;
    let j = (((z - c.grid_z0) / c.grid_step) as i32).clamp(0, c.grid_h as i32 - 1) as usize;
    c.y0 + heights[j * c.grid_w as usize + i] as f32 * c.height_step
}
