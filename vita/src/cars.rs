//! The traffic: every car in view as a box with its lamps, built into this
//! frame's vertices from where `tokyo_sim::traffic` has it. One draw.

use pocket_vita_gxm::mem::{Block as Memory, Kind, Ring};
use pocket_vita_gxm::program::{F32, S8N, U8N};
use tokyo_sim::math::*;
use tokyo_sim::traffic::Traffic;
use vita2d_sys as g;

use crate::gpu::{self, Blend, Cull, Gpu, Param, Program, Stream, Uniforms};
use tokyo_sim::mat::{self, Mat4};

const CAR_V: &str = include_str!("../shaders/car_v.cg");
const SOLID_F: &str = include_str!("../shaders/solid_f.cg");

/// Cars a frame draws at most, and how far from the eye.
const MOST: usize = 900;
const REACH: f32 = 1700.0;
/// Vertices and indices of one car: four sides, the roof, the lamps in front and behind.
const VERTS: usize = 28;
const INDICES: usize = 42;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 3],
    normal: [i8; 4],
    color: [u8; 4],
}

/// Paint, sRGB (`CAR_COLORS` in web/src/world/traffic.js).
const PAINT: [[u8; 3]; 10] = [[212, 212, 208], [212, 212, 208], [17, 18, 20], [17, 18, 20], [164, 167, 171], [164, 167, 171], [109, 114, 120], [31, 58, 110], [140, 28, 28], [196, 182, 143]];

pub struct Cars {
    prog: Program,
    mvp: Param,
    map: Param,
    frame: Param,
    look: Param,
    shadow: u32,
    _block: Memory,
    indices: *const u16,
    pub shown: u32,
}

impl Cars {
    /// # Safety
    /// GXM is initialized.
    pub unsafe fn new(gpu: &mut Gpu, defines: &str, msaa: u32) -> Result<Cars, String> {
        let attrs = [("aPosition", 0u16, F32, 3u8), ("aNormal", 12, S8N, 3), ("aColor", 16, U8N, 4)];
        let prog = gpu.program("car", defines, CAR_V, SOLID_F, &[Stream { stride: 20, instanced: false, attrs: &attrs }], msaa, [Blend::Opaque, Blend::Alpha])?;
        let mut block = Memory::with_access(Kind::Main, MOST * INDICES * 2 + 64, false)?;
        let indices = block.alloc(MOST * INDICES * 2, 16).ok_or("car indices")?.cast::<u16>();
        for c in 0..MOST {
            for q in 0..VERTS / 4 {
                let v = (c * VERTS + q * 4) as u16;
                for (k, o) in [0u16, 1, 2, 0, 2, 3].into_iter().enumerate() {
                    *indices.add(c * INDICES + q * 6 + k) = v + o;
                }
            }
        }
        Ok(Cars { mvp: prog.vs.param("uMvp"), map: prog.vs.param("uMap"), frame: prog.vs.param("uFrame"), look: prog.fs.param("uLook"), shadow: prog.fs.sampler_index("uShadow").unwrap_or(0), prog, _block: block, indices, shown: 0 })
    }

    /// Bytes of a frame's vertices.
    pub fn frame_bytes() -> usize {
        MOST * VERTS * core::mem::size_of::<Vertex>()
    }

    /// Builds and draws the cars in view.
    ///
    /// # Safety
    /// Inside a scene on `ctx`; `ring` is this frame's.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn draw(&mut self, ctx: *mut g::SceGxmContext, traffic: &Traffic, ring: &mut Ring, vp: &Mat4, eye: V3, map: &[f32; 4], frame: &[f32; 16], look: &[f32; 8], shadow: &g::SceGxmTexture) {
        self.shown = 0;
        let Some(out) = ring.alloc(Self::frame_bytes(), 16) else { return };
        let out = out.cast::<Vertex>();
        let planes = mat::planes(vp);
        let mut n = 0usize;
        for car in &traffic.cars {
            if n >= MOST {
                break;
            }
            let p = car.pos;
            if (p - eye).len2() > REACH * REACH || !mat::visible(&planes, &[p.x - 3.0, p.y, p.z - 3.0], &[p.x + 3.0, p.y + 2.0, p.z + 3.0]) {
                continue;
            }
            // Its size and paint stay with the car.
            let kind = car.look >> 8;
            let (half_l, half_w, h) = (1.7 + 0.7 * (kind & 3) as f32 / 3.0, 0.74 + 0.17 * ((kind >> 2) & 3) as f32 / 3.0, 1.4 + 0.5 * ((kind >> 4) & 3) as f32 / 3.0);
            let paint = PAINT[(kind >> 6) as usize % PAINT.len()];
            let f = v3(car.dir.x, 0.0, car.dir.z).norm_or(v3(1.0, 0.0, 0.0));
            let r = v3(-f.z, 0.0, f.x);
            let at = |along: f32, across: f32, up: f32| -> [f32; 3] {
                let q = p + f * along + r * across;
                [q.x, q.y + up + car.dir.y * along, q.z]
            };
            let n8 = |v: V3| [(v.x * 127.0) as i8, (v.y * 127.0) as i8, (v.z * 127.0) as i8, 0];
            let base = out.add(n * VERTS);
            let mut k = 0;
            let mut quad = |corners: [[f32; 3]; 4], normal: V3, color: [u8; 4]| {
                for c in corners {
                    *base.add(k) = Vertex { pos: c, normal: n8(normal), color };
                    k += 1;
                }
            };
            let body = [paint[0], paint[1], paint[2], 0];
            // (the glass: the roof and the upper sides read darker from the air)
            let roof = [(paint[0] as u16 * 3 / 5) as u8, (paint[1] as u16 * 3 / 5) as u8, (paint[2] as u16 * 3 / 5 + 12) as u8, 0];
            quad([at(-half_l, -half_w, h), at(half_l, -half_w, h), at(half_l, half_w, h), at(-half_l, half_w, h)], V3::UP, roof);
            quad([at(-half_l, half_w, 0.2), at(-half_l, half_w, h), at(half_l, half_w, h), at(half_l, half_w, 0.2)], r, body);
            quad([at(half_l, -half_w, 0.2), at(half_l, -half_w, h), at(-half_l, -half_w, h), at(-half_l, -half_w, 0.2)], -r, body);
            quad([at(half_l, half_w, 0.2), at(half_l, half_w, h), at(half_l, -half_w, h), at(half_l, -half_w, 0.2)], f, body);
            quad([at(-half_l, -half_w, 0.2), at(-half_l, -half_w, h), at(-half_l, half_w, h), at(-half_l, half_w, 0.2)], -f, body);
            // Lamps: a band across the nose and one across the tail, a hand in front of the paint.
            quad([at(half_l + 0.04, half_w, 0.45), at(half_l + 0.04, half_w, 0.95), at(half_l + 0.04, -half_w, 0.95), at(half_l + 0.04, -half_w, 0.45)], f, [255, 244, 214, 255]);
            quad([at(-half_l - 0.04, -half_w, 0.55), at(-half_l - 0.04, -half_w, 1.0), at(-half_l - 0.04, half_w, 1.0), at(-half_l - 0.04, half_w, 0.55)], -f, [230, 14, 8, 255]);
            n += 1;
        }
        self.shown = n as u32;
        if n == 0 {
            return;
        }
        self.prog.bind(ctx, false);
        gpu::state_opaque(ctx, Cull::None);
        g::sceGxmSetFragmentTexture(ctx, self.shadow, shadow);
        let v = Uniforms::vertex(ctx);
        v.set(self.mvp, vp);
        v.set(self.map, map);
        v.set(self.frame, frame);
        Uniforms::fragment(ctx).set(self.look, look);
        gpu::draw(ctx, out.cast(), self.indices, (n * INDICES) as u32);
    }
}
