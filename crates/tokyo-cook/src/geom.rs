//! Triangles, clipping and vertex welding.

use std::collections::HashMap;
use std::hash::Hash;

pub type P3 = [f32; 3];

pub fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn dot(a: P3, b: P3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn norm(a: P3) -> P3 {
    let l = dot(a, a).sqrt();
    if l > 1e-12 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}
/// Unit normal of a triangle and twice its area.
pub fn face(a: P3, b: P3, c: P3) -> (P3, f32) {
    let n = cross(sub(b, a), sub(c, a));
    let l = dot(n, n).sqrt();
    (norm(n), l)
}

/// A vertex with `N` numbers that interpolate along an edge: a position first, then whatever rides on it.
pub type V<const N: usize> = [f32; N];

fn lerp<const N: usize>(a: &V<N>, b: &V<N>, t: f32) -> V<N> {
    let mut o = [0.0; N];
    for i in 0..N {
        o[i] = a[i] + (b[i] - a[i]) * t;
    }
    o
}

/// The part of a convex polygon where `d(v) >= 0`.
pub fn clip<const N: usize>(poly: &[V<N>], d: impl Fn(&V<N>) -> f32) -> Vec<V<N>> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    if poly.is_empty() {
        return out;
    }
    let mut a = poly[poly.len() - 1];
    let mut da = d(&a);
    for b in poly {
        let db = d(b);
        if (da >= 0.0) != (db >= 0.0) {
            out.push(lerp(&a, b, da / (da - db)));
        }
        if db >= 0.0 {
            out.push(*b);
        }
        a = *b;
        da = db;
    }
    out
}

/// Cuts a triangle along the lines `v[axis] = k × period` and hands each piece to `emit` with the index `k` of
/// the band it lies in.
pub fn bands<const N: usize>(tri: &[V<N>; 3], axis: usize, period: f32, mut emit: impl FnMut(i32, [V<N>; 3])) {
    let lo = tri.iter().map(|v| v[axis]).fold(f32::INFINITY, f32::min);
    let hi = tri.iter().map(|v| v[axis]).fold(f32::NEG_INFINITY, f32::max);
    let eps = period * 1e-4;
    let k0 = ((lo + eps) / period).floor() as i32;
    let k1 = ((hi - eps) / period).floor() as i32;
    if k1 <= k0 {
        emit(k0, *tri);
        return;
    }
    for k in k0..=k1 {
        let (a, b) = (k as f32 * period, (k + 1) as f32 * period);
        let piece = clip(&clip(tri, |v| v[axis] - a), |v| b - v[axis]);
        for i in 1..piece.len().saturating_sub(1) {
            emit(k, [piece[0], piece[i], piece[i + 1]]);
        }
    }
}

/// Vertices and indices of one draw, welded as they are added.
pub struct Mesh<T: Copy + Eq + Hash> {
    pub vertices: Vec<T>,
    pub indices: Vec<u16>,
    seen: HashMap<T, u16>,
}

impl<T: Copy + Eq + Hash> Default for Mesh<T> {
    fn default() -> Self {
        Mesh { vertices: Vec::new(), indices: Vec::new(), seen: HashMap::new() }
    }
}

impl<T: Copy + Eq + Hash> Mesh<T> {
    /// Whether another triangle may need more vertices than an index can name.
    pub fn full(&self) -> bool {
        self.vertices.len() + 3 > 65535
    }

    /// Adds a triangle; `None` when it welds to a line and draws nothing.
    pub fn tri(&mut self, t: [T; 3]) -> Option<[u16; 3]> {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            return None;
        }
        let mut out = [0u16; 3];
        for (k, v) in t.into_iter().enumerate() {
            let next = self.vertices.len() as u16;
            let i = *self.seen.entry(v).or_insert(next);
            if i == next {
                self.vertices.push(v);
            }
            self.indices.push(i);
            out[k] = i;
        }
        Some(out)
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// `u8` of a value in 0..1.
pub fn unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub fn snorm8(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

pub fn srgb8(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    unorm8(if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 })
}
