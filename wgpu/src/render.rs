//! The wgpu renderer: the iPod touch's passes (`ipod/src/render.c`) from the
//! same pack, in a browser tab over WebGPU and on the build machine.
//!
//! A frame: the sky's dome, then the city in one pass with a depth buffer:
//! the ground and the roofs, the walls, what is painted, and the landmarks'
//! members, which are seen from both sides. The programs are
//! `shaders/city.wgsl`.
//!
//! What differs from the iPod touch's renderer, and why:
//!
//! - A vertex's place is read as four 16-bit numbers (the fourth is the two
//!   bytes that follow it and is not used): WebGPU has no format of three.
//! - A picture of 16 bits a texel goes to the GPU as `rgba8unorm`
//!   (`pocket_web_wgpu::picture`).
//! - Between two levels of a picture the GPU mixes them, and the ground is
//!   read with two samples along its slant. OpenGL ES takes the nearer level
//!   with those two samples; WebGPU allows several samples only with mixed
//!   levels.
//! - What changes from one place's draws to the next (the matrix, where the
//!   pictures lie) is a record of one buffer, written once a frame.
//! - Nothing is laid over the frame here: the interface is a pass of its own.

use bytemuck::{Pod, Zeroable};
use pocket_web_wgpu::gpu::{Gpu, Screen, DEPTH};
use pocket_web_wgpu::picture;
use pocket_web_wgpu::wgpu::{self, util::DeviceExt};
use tokyo_core::{SkyVertex, View, SLOTS};
use tokyo_pack::{kind, NearCell, KINDS, SECTORS};
use tokyo_sim::math::v3;
use tokyo_sim::view::{frame_of, Item};
use tokyo_sim::{mat, sky};

use crate::pack::{part, Stays, Tables, MAP_SIDE};

/// The eye keeps 30 m from what stands around it.
pub const NEAR_PLANE: f32 = 3.0;
pub const FAR_PLANE: f32 = 12000.0;
/// Haze per metre, and the most a point takes of it (the Vita's values).
const HAZE: f32 = 0.00022;
const HAZE_MOST: f32 = 0.94;
/// Records of `Place` a frame has room for at the start; the buffer grows when a frame needs more.
const PLACES: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    haze: [f32; 4],
    fog: [f32; 4],
    top: [f32; 4],
    wall: [f32; 4],
    solid: [f32; 4],
    lights: [[f32; 4]; SECTORS + 1],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PlaceUniform {
    /// A column after a column.
    mvp: [f32; 16],
    pic: [f32; 4],
    map: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Program {
    TopDay,
    TopDusk,
    TopNight,
    WallDay,
    WallNight,
    Solid,
    /// The solids' program without culling: the landmarks' members.
    Open,
    Sky,
}

/// Which picture a draw of the ground reads: a block's, or the one of a cell in a slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Picture {
    None,
    Block(u32),
    Slot(u32),
}

/// Where a draw's vertices and indices are.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Buffers {
    /// The levels that stay: this kind's vertices.
    City(usize),
    Slot(u32),
}

struct Draw {
    /// Its record of `Place`.
    place: u32,
    picture: Picture,
    buffers: Buffers,
    /// Where its kind's vertices start in the vertex buffer, bytes.
    vertices_at: u64,
    base_vertex: i32,
    first: u32,
    count: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub draws: u32,
    pub tris: [u32; KINDS],
}

struct Slot {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// The cell's picture of its own ground, once a cell has been in the slot, and its side.
    picture: Option<(wgpu::BindGroup, wgpu::Texture, u32)>,
}

pub struct Renderer {
    programs: Vec<wgpu::RenderPipeline>,
    /// The number of samples and the format the programs were built for.
    built_for: (wgpu::TextureFormat, u32),
    shader: wgpu::ShaderModule,
    layouts: Layouts,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    place_buffer: wgpu::Buffer,
    place_group: wgpu::BindGroup,
    /// Bytes from one record of `Place` to the next, and how many the buffer holds.
    place_step: usize,
    place_room: usize,
    city_vertices: [wgpu::Buffer; 3],
    city_indices: wgpu::Buffer,
    block_pictures: Vec<wgpu::BindGroup>,
    facades: wgpu::BindGroup,
    ground_sampler: wgpu::Sampler,
    shadows: wgpu::Texture,
    slots: Vec<Slot>,
    sky_vertices: wgpu::Buffer,
    sky_indices: wgpu::Buffer,
    sky_hour: f32,
    // A frame's records, kept from one frame to the next for their memory.
    places: Vec<PlaceUniform>,
    draws: [Vec<Draw>; KINDS],
    staging: Vec<u8>,
}

struct Layouts {
    frame: wgpu::BindGroupLayout,
    place: wgpu::BindGroupLayout,
    picture: wgpu::BindGroupLayout,
    facades: wgpu::BindGroupLayout,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }
}

fn uniform_entry(binding: u32, size: u64, moves: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: moves, min_binding_size: wgpu::BufferSize::new(size) },
        count: None,
    }
}

fn view_of(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A matrix of `tokyo_sim::mat` (a row after a row) as WGSL stores one (a column after a column).
fn columns(m: &mat::Mat4) -> [f32; 16] {
    core::array::from_fn(|i| m[(i % 4) * 4 + i / 4])
}

impl Layouts {
    fn new(gpu: &Gpu) -> Layouts {
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries });
        Layouts {
            frame: layout("frame", &[uniform_entry(0, size_of::<FrameUniform>() as u64, false), sampler_entry(1), texture_entry(2), texture_entry(3)]),
            place: layout("place", &[uniform_entry(0, size_of::<PlaceUniform>() as u64, true)]),
            picture: layout("picture", &[texture_entry(0), sampler_entry(1)]),
            facades: layout("facades", &[texture_entry(0), sampler_entry(1), texture_entry(2)]),
        }
    }
}

fn programs(gpu: &Gpu, shader: &wgpu::ShaderModule, layouts: &Layouts, format: wgpu::TextureFormat, samples: u32) -> Vec<wgpu::RenderPipeline> {
    use wgpu::VertexFormat::*;
    let attribute = |shader_location, format, offset| wgpu::VertexAttribute { format, offset, shader_location };
    // (the place is three 16-bit numbers; the fourth read with them is the vertex's next two bytes)
    let top = [attribute(0, Sint16x4, 0), attribute(1, Uint8x4, 4)];
    let wall = [attribute(0, Sint16x4, 0), attribute(1, Uint8x4, 4), attribute(2, Uint8x4, 8), attribute(3, Sint16x2, 12)];
    let solid = [attribute(0, Sint16x4, 0), attribute(1, Uint8x4, 4), attribute(2, Uint8x4, 8)];
    let dome = [attribute(0, Float32x3, 0), attribute(1, Unorm8x4, 12)];
    let layout = |label, groups: &[&wgpu::BindGroupLayout]| gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: groups, push_constant_ranges: &[] });
    let ground = layout("ground", &[&layouts.frame, &layouts.place, &layouts.picture]);
    let walls = layout("walls", &[&layouts.frame, &layouts.place, &layouts.facades]);
    let plain = layout("plain", &[&layouts.frame, &layouts.place]);
    let build = |label: &str, layout: &wgpu::PipelineLayout, vertex: &str, fragment: &str, stride: u64, attributes: &[wgpu::VertexAttribute], cull: Option<wgpu::Face>, depth: bool| {
        gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vertex),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout { array_stride: stride, step_mode: wgpu::VertexStepMode::Vertex, attributes }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, front_face: wgpu::FrontFace::Ccw, cull_mode: cull, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: depth,
                depth_compare: if depth { wgpu::CompareFunction::Less } else { wgpu::CompareFunction::Always },
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        })
    };
    let back = Some(wgpu::Face::Back);
    // (in the order of `Program`)
    vec![
        build("top day", &ground, "top_vertex", "top_day", 8, &top, back, true),
        build("top dusk", &ground, "top_vertex", "top_dusk", 8, &top, back, true),
        build("top night", &ground, "top_vertex", "top_night", 8, &top, back, true),
        build("wall day", &walls, "wall_vertex", "wall_day", 16, &wall, back, true),
        build("wall night", &walls, "wall_vertex", "wall_night", 16, &wall, back, true),
        build("solid", &plain, "solid_vertex", "solid", 12, &solid, back, true),
        build("open", &plain, "solid_vertex", "solid", 12, &solid, None, true),
        build("sky", &plain, "sky_vertex", "sky", 16, &dome, None, false),
    ]
}

impl Renderer {
    /// Hands what the pack keeps in memory to the GPU.
    pub fn new(gpu: &Gpu, screen: &Screen, tables: &Tables, stays: &Stays) -> Result<Renderer, String> {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("city"), source: wgpu::ShaderSource::Wgsl(include_str!("shaders/city.wgsl").into()) });
        let layouts = Layouts::new(gpu);
        let buffer = |label, contents: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage });
        let empty = |label, size: u64, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        // (a buffer's size is a multiple of four bytes: an odd count of indices gets two more)
        let padded = |bytes: &[u8]| {
            let mut v = bytes.to_vec();
            v.resize((v.len() + 3) & !3, 0);
            v
        };
        let city_vertices = [0, 1, 2].map(|k| buffer("city vertices", &padded(stays.vertices[k]), wgpu::BufferUsages::VERTEX));
        let city_indices = buffer("city indices", &padded(stays.indices), wgpu::BufferUsages::INDEX);

        let sampler = |label, v: wgpu::AddressMode, several: u16| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: v,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                anisotropy_clamp: several,
                ..Default::default()
            })
        };
        // The ground is seen at a slant: two samples along it keep a level finer.
        let ground_sampler = sampler("ground", wgpu::AddressMode::ClampToEdge, 2);
        // A facade's picture runs once across a wall and repeats up it.
        let facade_sampler = sampler("facades", wgpu::AddressMode::Repeat, 1);
        let map_sampler = sampler("over the city", wgpu::AddressMode::ClampToEdge, 1);

        let blocks = tables.blocks.len();
        let texels = |p: &tokyo_pack::HandPicture, at: usize| -> Result<&[u8], String> {
            let size = picture::stored_bytes(p.width as u32, p.height as u32, p.levels);
            stays.texels.get(p.offset as usize + at..p.offset as usize + at + size).ok_or_else(|| "a picture of the pack is cut short".into())
        };
        let mut block_pictures = Vec::with_capacity(blocks);
        for p in &stays.pictures[..blocks] {
            let texture = picture::create(gpu, "block", p.width as u32, p.height as u32);
            picture::write(gpu, &texture, texels(p, 0)?, p.width as u32, p.height as u32, p.levels);
            block_pictures.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("block"),
                layout: &layouts.picture,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_of(&texture)) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&ground_sampler) }],
            }));
        }
        // The facades by day, then what their windows emit: two pictures of one size, one after the other.
        let f = &stays.pictures[blocks];
        let pair = picture::stored_bytes(f.width as u32, f.height as u32, f.levels);
        let facade = [0, 1].map(|k| -> Result<wgpu::Texture, String> {
            let texture = picture::create(gpu, "facades", f.width as u32, f.height as u32);
            picture::write(gpu, &texture, texels(f, k * pair)?, f.width as u32, f.height as u32, f.levels);
            Ok(texture)
        });
        let [day, night] = facade;
        let facades = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("facades"),
            layout: &layouts.facades,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_of(&day?)) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&facade_sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view_of(&night?)) },
            ],
        });
        let lamps = picture::create(gpu, "lamps", MAP_SIDE, MAP_SIDE);
        picture::write(gpu, &lamps, stays.lamps, MAP_SIDE, MAP_SIDE, 1);
        // Until the first sweep: everything in the sun.
        let shadows = picture::create_bytes(gpu, "shadows", MAP_SIDE);
        picture::write_bytes(gpu, &shadows, MAP_SIDE, 0, MAP_SIDE, &vec![255u8; (MAP_SIDE * MAP_SIDE) as usize]);

        let frame_buffer = empty("frame", size_of::<FrameUniform>() as u64, wgpu::BufferUsages::UNIFORM);
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &layouts.frame,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&map_sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view_of(&lamps)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view_of(&shadows)) },
            ],
        });
        let place_step = (size_of::<PlaceUniform>()).next_multiple_of(device.limits().min_uniform_buffer_offset_alignment as usize);
        let (place_buffer, place_group) = Self::place_room(gpu, &layouts, place_step, PLACES);

        // A slot holds any cell's record: the largest sizes of the pack.
        let most = |f: &dyn Fn(&NearCell) -> usize| tables.near.iter().map(f).max().unwrap_or(0).max(16) as u64;
        let (vertex_room, index_room) = (most(&|n| part(n, 3)), most(&|n| (n.parts[3] as usize + 15) & !15));
        let slots = (0..SLOTS)
            .map(|_| Slot { vertices: empty("slot vertices", vertex_room, wgpu::BufferUsages::VERTEX), indices: empty("slot indices", index_room, wgpu::BufferUsages::INDEX), picture: None })
            .collect();

        let mut dome = [0u16; sky::DOME_INDICES];
        unsafe { tokyo_core::tk_sky_indices(dome.as_mut_ptr()) };
        let sky_indices = buffer("sky indices", bytemuck::cast_slice(&dome), wgpu::BufferUsages::INDEX);
        let sky_vertices = empty("sky vertices", (sky::DOME_VERTS * size_of::<SkyVertex>()) as u64, wgpu::BufferUsages::VERTEX);

        Ok(Renderer {
            programs: programs(gpu, &shader, &layouts, screen.format, screen.samples),
            built_for: (screen.format, screen.samples),
            shader,
            layouts,
            frame_buffer,
            frame_group,
            place_buffer,
            place_group,
            place_step,
            place_room: PLACES,
            city_vertices,
            city_indices,
            block_pictures,
            facades,
            ground_sampler,
            shadows,
            slots,
            sky_vertices,
            sky_indices,
            sky_hour: -1.0,
            places: Vec::with_capacity(PLACES),
            draws: core::array::from_fn(|_| Vec::with_capacity(512)),
            staging: Vec::new(),
        })
    }

    fn place_room(gpu: &Gpu, layouts: &Layouts, step: usize, room: usize) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("places"), size: (step * room) as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("place"),
            layout: &layouts.place,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(size_of::<PlaceUniform>() as u64) }) }],
        });
        (buffer, group)
    }

    /// A cell's record that has arrived for a slot: its vertices, indices and picture go to the GPU.
    pub fn cell(&mut self, gpu: &Gpu, slot: usize, record: &NearCell, bytes: &[u8]) -> Result<(), String> {
        let (vertices, indices, pixels) = (part(record, 3), (record.parts[3] as usize + 3) & !3, part(record, 4));
        let (side, levels) = (record.width as u32, record.levels as u32);
        if bytes.len() < pixels + picture::stored_bytes(side, side, levels) || bytes.len() < vertices + indices {
            return Err("a cell's record is cut short".into());
        }
        let s = &mut self.slots[slot];
        gpu.queue.write_buffer(&s.vertices, 0, &bytes[..vertices]);
        gpu.queue.write_buffer(&s.indices, 0, &bytes[vertices..vertices + indices]);
        // (a slot's texture keeps its storage from one cell to the next)
        if s.picture.as_ref().map(|p| p.2) != Some(side) {
            let texture = picture::create(gpu, "cell", side, side);
            let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cell"),
                layout: &self.layouts.picture,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view_of(&texture)) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.ground_sampler) }],
            });
            s.picture = Some((group, texture, side));
        }
        if let Some((_, texture, _)) = &s.picture {
            picture::write(gpu, texture, &bytes[pixels..], side, side, levels);
        }
        Ok(())
    }

    /// A sweep of the shadows: `MAP_SIDE` squared texels of 8 bits, in rows. Frames read it from the next on.
    pub fn shadows(&mut self, gpu: &Gpu, texels: &[u8]) {
        picture::write_bytes(gpu, &self.shadows, MAP_SIDE, 0, MAP_SIDE, texels);
    }

    /// One kind's draws, as the lists give them: a record of `Place` when the place or the picture changes.
    fn list(&mut self, k: usize, vp: &mat::Mat4, tables: &Tables, items: &[Item], stats: &mut Stats) {
        let c = &tables.city;
        let (mut placed, mut bound) = ((u32::MAX, false), Picture::None);
        let mut record = PlaceUniform::zeroed();
        const A: f32 = 1.0 / 65535.0;
        const HALF: f32 = 32768.0 / 65535.0;
        self.draws[k].clear();
        for it in items {
            let b = &tables.batches[it.batch as usize];
            let near = it.cell != u32::MAX;
            let (buffers, vertices_at) = if near {
                let slot = tokyo_core::tk_slot_of(it.cell);
                if slot < 0 {
                    continue;
                }
                (Buffers::Slot(slot as u32), part(&tables.near[it.cell as usize], k) as u64)
            } else {
                // (a landmark's members are painted geometry: the solids' vertices)
                (Buffers::City(if k == kind::OPEN as usize { kind::SOLID as usize } else { k }), 0)
            };
            let mut fresh = false;
            if placed != (it.place, it.region) {
                placed = (it.place, it.region);
                let (origin, span) = frame_of(c, it.place, it.region, k == kind::TOP as usize);
                // clip = vp x T(origin + span / 2) x S(span / 65535)
                record.mvp = columns(&mat::with_bounds(vp, [0, 1, 2].map(|i| origin[i] + span[i] * HALF), [0, 1, 2].map(|i| span[i] * A)));
                // The lamps' light and the shadows lie over the grid of heights.
                let side = MAP_SIDE as f32 * c.grid_step;
                let a = span[0] * A / side;
                record.map = [a, (origin[0] + span[0] * HALF - c.grid_x0) / side, a, (origin[2] + span[2] * HALF - c.grid_z0) / side];
                fresh = true;
            }
            if k == kind::TOP as usize {
                // A cell near the eye has a picture of its own; the others take their place's.
                let want = match buffers {
                    Buffers::Slot(slot) => Picture::Slot(slot),
                    Buffers::City(_) => Picture::Block(it.place),
                };
                if bound != want || fresh {
                    bound = want;
                    record.pic = match want {
                        Picture::Slot(_) => {
                            let n = c.cells as f32;
                            let within = it.cell % (c.cells * c.cells);
                            [A * n, HALF * n - (within % c.cells) as f32, A * n, HALF * n - (within / c.cells) as f32]
                        }
                        _ => [A, HALF, A, HALF],
                    };
                    fresh = true;
                }
            }
            if fresh {
                self.places.push(record);
            }
            self.draws[k].push(Draw { place: self.places.len() as u32 - 1, picture: bound, buffers, vertices_at, base_vertex: b.vtx_first as i32, first: b.idx_first + it.from, count: it.to - it.from });
            stats.draws += 1;
            stats.tris[k] += (it.to - it.from) / 3;
        }
    }

    /// One frame into the screen. `lists`: the frame's draws by kind, as the core chose them for this screen's
    /// shape.
    pub fn frame(&mut self, gpu: &Gpu, screen: &Screen, tables: &Tables, view: &View, lists: [&[Item]; KINDS]) -> Result<Stats, String> {
        if self.built_for != (screen.format, screen.samples) {
            self.programs = programs(gpu, &self.shader, &self.layouts, screen.format, screen.samples);
            self.built_for = (screen.format, screen.samples);
        }
        let mut stats = Stats::default();
        let eye = v3(view.eye[0], view.eye[1], view.eye[2]);
        let look = v3(view.look[0], view.look[1], view.look[2]).norm_or(v3(0.0, 0.0, -1.0));
        let aspect = screen.width as f32 / screen.height as f32;
        let vp = mat::mul(&mat::perspective(view.fov, aspect, NEAR_PLANE, FAR_PLANE), &mat::view(eye, look, 0.0));

        // ---- the sky: a dome around the eye, coloured at its vertices for the hour
        if (view.hour - self.sky_hour).abs() > 0.004 {
            self.sky_hour = view.hour;
            let mut dome = [const { core::mem::MaybeUninit::<SkyVertex>::uninit() }; sky::DOME_VERTS];
            unsafe {
                tokyo_core::tk_sky(dome.as_mut_ptr().cast(), 900.0);
                gpu.queue.write_buffer(&self.sky_vertices, 0, core::slice::from_raw_parts(dome.as_ptr().cast::<u8>(), size_of_val(&dome)));
            }
        }
        self.places.clear();
        self.places.push(PlaceUniform { mvp: columns(&mat::translated(&vp, eye)), pic: [0.0; 4], map: [0.0; 4] });
        stats.draws += 1;

        // ---- the city
        let night = view.night;
        let frame = FrameUniform {
            haze: [HAZE, if view.option & 8 != 0 { 0.0 } else { HAZE_MOST }, 0.0, 0.0],
            fog: [view.haze[0], view.haze[1], view.haze[2], 0.8 * night],
            top: [view.top[0], view.top[1], view.top[2], 1.0 / 255.0],
            // (FACADE_V repeats of the picture up a wall over the signed range)
            wall: [1.0 / 32767.0, tokyo_pack::FACADE_V / 32767.0, 1.0 / 255.0, night],
            // (w: how far a lamp shines by itself, at the half scale light travels at)
            solid: [0.0, 0.0, 1.0 / 255.0, 0.5 * night],
            lights: view.lights.map(|l| [l[0], l[1], l[2], 0.0]),
        };
        gpu.queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        for (k, items) in lists.into_iter().enumerate() {
            // (`option`: a kind left out, for a measurement)
            let shown = view.option & (32 << k) == 0;
            self.list(k, &vp, tables, if shown { items } else { &[] }, &mut stats);
        }
        if self.places.len() > self.place_room {
            self.place_room = self.places.len().next_power_of_two();
            (self.place_buffer, self.place_group) = Self::place_room(gpu, &self.layouts, self.place_step, self.place_room);
        }
        self.staging.clear();
        self.staging.resize(self.places.len() * self.place_step, 0);
        for (i, place) in self.places.iter().enumerate() {
            self.staging[i * self.place_step..i * self.place_step + size_of::<PlaceUniform>()].copy_from_slice(bytemuck::bytes_of(place));
        }
        gpu.queue.write_buffer(&self.place_buffer, 0, &self.staging);

        let target = screen.frame(gpu)?;
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            // (what nothing of the scene covers, under the dome's lowest ring and past the city's edge, is haze)
            let mut pass = target.pass(&mut encoder, view.haze);
            let step = self.place_step as u32;
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_pipeline(&self.programs[Program::Sky as usize]);
            pass.set_bind_group(1, &self.place_group, &[0]);
            pass.set_vertex_buffer(0, self.sky_vertices.slice(..));
            pass.set_index_buffer(self.sky_indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..sky::DOME_INDICES as u32, 0, 0..1);

            // By day no lamp is on; deep in the night no shadow is cast.
            let top = if night < 0.004 { Program::TopDay } else if night > 0.97 { Program::TopNight } else { Program::TopDusk };
            let wall = if night < 0.004 { Program::WallDay } else { Program::WallNight };
            for (k, program) in [top, wall, Program::Solid, Program::Open].into_iter().enumerate() {
                if self.draws[k].is_empty() {
                    continue;
                }
                pass.set_pipeline(&self.programs[program as usize]);
                if k == kind::WALL as usize {
                    pass.set_bind_group(2, &self.facades, &[]);
                }
                let (mut placed, mut bound, mut vertices, mut indices) = (u32::MAX, Picture::None, None, None);
                for d in &self.draws[k] {
                    if placed != d.place {
                        placed = d.place;
                        pass.set_bind_group(1, &self.place_group, &[d.place * step]);
                    }
                    if k == kind::TOP as usize && bound != d.picture {
                        let group = match d.picture {
                            Picture::Block(block) => self.block_pictures.get(block as usize),
                            Picture::Slot(slot) => self.slots[slot as usize].picture.as_ref().map(|p| &p.0),
                            Picture::None => None,
                        };
                        // (a draw whose picture is not there is left out)
                        let Some(group) = group else { continue };
                        bound = d.picture;
                        pass.set_bind_group(2, group, &[]);
                    }
                    if vertices != Some((d.buffers, d.vertices_at)) {
                        vertices = Some((d.buffers, d.vertices_at));
                        let buffer = match d.buffers {
                            Buffers::City(kind) => &self.city_vertices[kind],
                            Buffers::Slot(slot) => &self.slots[slot as usize].vertices,
                        };
                        pass.set_vertex_buffer(0, buffer.slice(d.vertices_at..));
                    }
                    if indices != Some(d.buffers) {
                        indices = Some(d.buffers);
                        let buffer = match d.buffers {
                            Buffers::City(_) => &self.city_indices,
                            Buffers::Slot(slot) => &self.slots[slot as usize].indices,
                        };
                        pass.set_index_buffer(buffer.slice(..), wgpu::IndexFormat::Uint16);
                    }
                    pass.draw_indexed(d.first..d.first + d.count, d.base_vertex, 0..1);
                }
            }
        }
        gpu.queue.submit([encoder.finish()]);
        target.present();
        Ok(stats)
    }
}
