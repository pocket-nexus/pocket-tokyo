//! What a tile holds while it is being compiled: triangles in the world's
//! frame, by the program that will draw them.

use crate::geom::{face, P3};

/// Ground or roof.
#[derive(Clone, Copy)]
pub struct TopTri {
    pub p: [P3; 3],
    /// Per vertex: the ground is smooth, a roof flat.
    pub n: [P3; 3],
}

impl TopTri {
    pub fn flat(p: [P3; 3]) -> TopTri {
        let (n, _) = face(p[0], p[1], p[2]);
        TopTri { p, n: [n; 3] }
    }
}

#[derive(Clone, Copy)]
pub struct WallTri {
    pub p: [P3; 3],
    pub n: P3,
    /// In the facade picture: `u` across it, `v` in repeats of it.
    pub uv: [[f32; 2]; 3],
    pub color: [u8; 3],
    pub gain: u8,
    pub late: u8,
    /// Which way the wall faces, for drawing only the walls that face the eye (`sector_of`).
    pub sector: u8,
}

use tokyo_pack::SECTORS;

pub fn sector_of(n: P3) -> u8 {
    if n[1].abs() > 0.5 {
        return SECTORS as u8;
    }
    let turn = (n[2].atan2(n[0]) + std::f32::consts::PI) / std::f32::consts::TAU;
    ((turn * SECTORS as f32) as usize).min(SECTORS - 1) as u8
}

/// The group a painted triangle is lit in: the sector it faces when it stands upright, the rest together.
pub fn solid_sector(p: &[P3; 3]) -> u8 {
    let (n, _) = face(p[0], p[1], p[2]);
    if n[1].abs() >= 0.2 {
        return SECTORS as u8;
    }
    sector_of(n)
}

#[derive(Clone, Copy)]
pub struct SolidTri {
    pub p: [P3; 3],
    /// Per corner: a face of a structure is flat, a tree's crown round.
    pub n: [P3; 3],
    /// sRGB; alpha: how far the face shines by its own colour at night (255 a lamp, less under floodlights).
    pub color: [u8; 4],
}

/// One level of detail of a tile.
#[derive(Default, Clone)]
pub struct Level {
    pub tops: Vec<TopTri>,
    pub walls: Vec<WallTri>,
    pub solids: Vec<SolidTri>,
}

/// The tile a point lies in.
pub fn tile_of(x: f32, z: f32, tile: f32) -> (i32, i32) {
    ((x / tile).floor() as i32, (z / tile).floor() as i32)
}
