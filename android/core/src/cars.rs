//! The traffic: every car in view as a box with its lamps, built into this
//! frame's vertices from where `tokyo_sim::traffic` has it. One draw.

use core::ffi::c_void;
use tokyo_sim::mat::{self, Mat4};
use tokyo_sim::math::*;
use tokyo_sim::traffic::Traffic;

use crate::city::{Light, Pass, SOLID_F};
use crate::gl;

const CAR_V: &str = include_str!("../../shaders/car.vert");

/// Cars a frame draws at most, and how far from the eye.
const MOST: usize = 900;
const REACH: f32 = 1700.0;
/// Vertices and indices of one car: four sides, the roof, the lamps in front and behind.
const VERTS: usize = 28;
const INDICES: usize = 42;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vertex {
    pos: [f32; 3],
    normal: [i8; 4],
    color: [u8; 4],
}

/// Paint, sRGB (`CAR_COLORS` in web/src/world/traffic.js).
const PAINT: [[u8; 3]; 10] = [[212, 212, 208], [212, 212, 208], [17, 18, 20], [17, 18, 20], [164, 167, 171], [164, 167, 171], [109, 114, 120], [31, 58, 110], [140, 28, 28], [196, 182, 143]];

pub struct Cars {
    pass: Pass,
    array: u32,
    buffer: u32,
    vertices: Vec<Vertex>,
    pub shown: u32,
}

impl Cars {
    /// # Safety
    /// A current context.
    pub unsafe fn new(defines: &str) -> Result<Cars, String> {
        let pass = Pass::new("car", defines, CAR_V, SOLID_F, &["aPosition", "aNormal", "aColor"])?;
        let mut indices = vec![0u16; MOST * INDICES];
        for c in 0..MOST {
            for q in 0..VERTS / 4 {
                let v = (c * VERTS + q * 4) as u16;
                for (k, o) in [0u16, 1, 2, 0, 2, 3].into_iter().enumerate() {
                    indices[c * INDICES + q * 6 + k] = v + o;
                }
            }
        }
        let (mut array, mut buffers) = (0, [0u32; 2]);
        gl::glGenVertexArrays(1, &mut array);
        gl::glGenBuffers(2, buffers.as_mut_ptr());
        gl::glBindVertexArray(array);
        gl::glBindBuffer(gl::ARRAY_BUFFER, buffers[0]);
        gl::glBindBuffer(gl::ELEMENT_ARRAY_BUFFER, buffers[1]);
        gl::glBufferData(gl::ELEMENT_ARRAY_BUFFER, (indices.len() * 2) as isize, indices.as_ptr().cast(), gl::STATIC_DRAW);
        let stride = core::mem::size_of::<Vertex>() as i32;
        gl::glEnableVertexAttribArray(0);
        gl::glVertexAttribPointer(0, 3, gl::FLOAT, 0, stride, core::ptr::null());
        gl::glEnableVertexAttribArray(1);
        gl::glVertexAttribPointer(1, 4, gl::BYTE, 1, stride, 12 as *const c_void);
        gl::glEnableVertexAttribArray(2);
        gl::glVertexAttribPointer(2, 4, gl::UNSIGNED_BYTE, 1, stride, 16 as *const c_void);
        gl::glBindVertexArray(0);
        Ok(Cars { pass, array, buffer: buffers[0], vertices: Vec::with_capacity(MOST * VERTS), shown: 0 })
    }

    /// Builds and draws the cars in view. `planes`: the frustum.
    ///
    /// # Safety
    /// A current context with the frame's target bound.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn draw(&mut self, traffic: &Traffic, planes: &[[f32; 4]; 6], vp: &Mat4, eye: V3, map: &[f32; 4], frame: &[f32; 24], shadow: u32) {
        self.vertices.clear();
        let mut n = 0usize;
        for car in &traffic.cars {
            if n >= MOST {
                break;
            }
            let p = car.pos;
            if (p - eye).len2() > REACH * REACH || !mat::visible(planes, &[p.x - 3.0, p.y, p.z - 3.0], &[p.x + 3.0, p.y + 2.0, p.z + 3.0]) {
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
            let vertices = &mut self.vertices;
            let mut quad = |corners: [[f32; 3]; 4], normal: V3, color: [u8; 4]| {
                for c in corners {
                    vertices.push(Vertex { pos: c, normal: n8(normal), color });
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
        self.pass.begin(frame, &Light::default());
        self.pass.place(vp, map);
        gl::bind(1, shadow);
        gl::glDisable(gl::CULL_FACE);
        gl::glBindVertexArray(self.array);
        gl::glBindBuffer(gl::ARRAY_BUFFER, self.buffer);
        // A buffer of its own each frame: the driver does not wait for the frame that still reads the last.
        let bytes = (self.vertices.len() * core::mem::size_of::<Vertex>()) as isize;
        gl::glBufferData(gl::ARRAY_BUFFER, bytes, self.vertices.as_ptr().cast(), gl::STREAM_DRAW);
        gl::glDrawElements(gl::TRIANGLES, (n * INDICES) as i32, gl::UNSIGNED_SHORT, core::ptr::null());
        gl::glBindVertexArray(0);
    }
}
