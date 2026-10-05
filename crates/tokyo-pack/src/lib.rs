//! The device pack (`TKPK`): what the city compiler writes and a runtime reads.
//!
//! Little-endian. Header: `"TKPK"`, version, section count, zero; then one
//! 16-byte entry per section (tag, offset, size, zero). Sections start on
//! 16-byte boundaries. A runtime reads the header and the table, then each
//! section it wants; `GTEX` is read a picture at a time.
//!
//! The city is cut three ways. A *block* is a square of ground with one
//! picture from above; it is drawn whole at the mid level of detail. A block
//! is `City::cells` squared *cells*, each drawn on its own at the near level.
//! Blocks stand two by two in *regions*, each drawn whole, with a small
//! picture of its own, at the far level.
//!
//! | Tag    | Contents |
//! | ------ | -------- |
//! | `META` | JSON: identity, sources, statistics, the profile; for tools |
//! | `CITY` | `City`: bounds, grids and quantization of everything else |
//! | `REGN` | `Region` table, row by row from the north-west |
//! | `BLCK` | `Block` table, row by row |
//! | `CELL` | `Cell` table: `City::cells` squared per block, row by row |
//! | `BTCH` | `Batch` table |
//! | `VTOP` | `TopVertex` records: the ground and every roof |
//! | `VWAL` | `WallVertex` records |
//! | `VSOL` | `SolidVertex` records: structures, models, the lattice tower |
//! | `IDX0` | `u16` indices, relative to each batch's first vertex |
//! | `SPAN` | `u32` offsets into a batch's indices, per batch ordered by cell: `cells² + 1`, or `cells² × (SECTORS + 1) + 1` for walls |
//! | `FACD` | the facades by day: `TexHeader`, then BC3 levels, largest first |
//! | `FACN` | the facades' lights: `TexHeader`, then BC1 levels |
//! | `GLVL` | `GroundLevel` table: where each picture from above is in `GTEX` |
//! | `GTEX` | BC1 levels of every block's and every region's picture from above |
//! | `HMAP` | `u16` per cell of `City::grid`: the top of whatever stands there, in `City::height_step` metres above `City::y0` |
//! | `TOUR` | `f32 × 6` per place of the tour: where the eye is, and the point it looks at |
//! | `LANE` | the traffic's lanes: `tokyo_sim::traffic::Lane` records |
//! | `LPTS` | `f32 × 3` per point of a lane, on its side of the road |
//! | `LAMP` | `u16` per cell of the same grid: lamp light on the ground at night, half strength, `r << 11 | g << 5 | b` |

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const MAGIC: u32 = u32::from_le_bytes(*b"TKPK");
pub const VERSION: u32 = 2;

pub const fn tag(t: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*t)
}
pub const META: u32 = tag(b"META");
pub const CITY: u32 = tag(b"CITY");
pub const REGN: u32 = tag(b"REGN");
pub const BLCK: u32 = tag(b"BLCK");
pub const CELL: u32 = tag(b"CELL");
pub const BTCH: u32 = tag(b"BTCH");
pub const VTOP: u32 = tag(b"VTOP");
pub const VWAL: u32 = tag(b"VWAL");
pub const VSOL: u32 = tag(b"VSOL");
pub const IDX0: u32 = tag(b"IDX0");
pub const SPAN: u32 = tag(b"SPAN");
pub const FACD: u32 = tag(b"FACD");
pub const FACN: u32 = tag(b"FACN");
pub const GLVL: u32 = tag(b"GLVL");
pub const GTEX: u32 = tag(b"GTEX");
pub const HMAP: u32 = tag(b"HMAP");
pub const LAMP: u32 = tag(b"LAMP");
pub const TOUR: u32 = tag(b"TOUR");
pub const LANE: u32 = tag(b"LANE");
pub const LPTS: u32 = tag(b"LPTS");

/// Levels of detail: near by cell, mid by block, far by region.
pub const LODS: usize = 3;
/// Blocks along a region's side.
pub const REGION_BLOCKS: usize = 2;

/// The city's frame: metres, x east, y up, z south.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct City {
    /// Side of a block.
    pub block: f32,
    /// Blocks along x and z, and the corner of the north-west one.
    pub blocks_x: u32,
    pub blocks_z: u32,
    pub x0: f32,
    pub z0: f32,
    /// Cells along a block's side.
    pub cells: u32,
    /// Heights dequantize as `y0 + q / 65535 × y_span`.
    pub y0: f32,
    pub y_span: f32,
    /// Walls and solids reach this far beyond their block or region: x and z dequantize as
    /// `-margin + q / 65535 × (side + 2 × margin)`.
    pub margin: f32,
    /// The height grid (`HMAP`, `LAMP`): its corner, cell size and extent.
    pub grid_x0: f32,
    pub grid_z0: f32,
    pub grid_step: f32,
    pub grid_w: u32,
    pub grid_h: u32,
    /// Metres per unit of `HMAP`.
    pub height_step: f32,
    /// Where the camera starts: position and the point looked at.
    pub view: [f32; 6],
    /// The hour the city opens at.
    pub hour: f32,
    pub pad: [u32; 2],
}

/// Which program draws a batch.
pub mod kind {
    /// The ground and roofs: `TopVertex`, textured by a picture from above.
    pub const TOP: u32 = 0;
    /// Walls: `WallVertex`, textured by the facade pictures.
    pub const WALL: u32 = 1;
    /// Painted geometry: `SolidVertex`.
    pub const SOLID: u32 = 2;
}

/// A range of the `BTCH` table.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Draws {
    pub first: u32,
    pub count: u32,
}

/// Two by two blocks at the far level of detail. Its vertices are normalized over the region's side.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Region {
    /// Lowest and highest point of anything in the region.
    pub y_min: f32,
    pub y_max: f32,
    /// Its batches, their indices ordered by cell: `City::cells` squared cells over the region.
    pub far: Draws,
    /// `GroundLevel` records of its picture from above, largest first. The picture also shows what the far
    /// level does not draw: structures, railways, bridges.
    pub ground_first: u32,
    pub ground_levels: u32,
}

/// A square of the city with one picture from above. Its vertices, and its cells', are normalized over its side.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Block {
    pub y_min: f32,
    pub y_max: f32,
    /// The mid level of detail: batches with their indices ordered by cell, so a frame draws the cells in view
    /// that are not drawn at the near level.
    pub mid: Draws,
    pub ground_first: u32,
    pub ground_levels: u32,
}

/// A square part of a block: its bounds, and what is drawn there near the eye.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Cell {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub near: Draws,
}

/// One draw: a range of vertices and indices and its bounds in the world.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Batch {
    pub kind: u32,
    pub vtx_first: u32,
    pub vtx_count: u32,
    pub idx_first: u32,
    pub idx_count: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Where the batch's record starts in `SPAN`; `u32::MAX` when its indices are in no order.
    pub spans: u32,
}

/// A batch ordered by cell has its indices grouped by the cell of its place they lie in, row by row; its `SPAN`
/// record is where each cell starts, and the end. A batch of walls is ordered within each cell by the sector
/// of the compass the wall faces (sector `k` holds normals whose angle `atan2(z, x)` lies in
/// `-π + k × 2π / SECTORS` and the next), then the faces that look up or down: its record has
/// `SECTORS + 1` entries per cell. A frame draws, of each cell in view, only the arc that can face the eye.
pub const SECTORS: usize = 16;

/// One level of a picture from above: a square of BC1 blocks in `GTEX`.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct GroundLevel {
    pub offset: u32,
    pub size: u32,
    pub width: u32,
    pub pad: u32,
}

/// Ground or roof, 12 bytes. `pos`: x and z over the block (or region), y over the city's height range, all
/// `u16` normalized; x and z are also the texture coordinates. `ao`: how much of the sky the point sees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct TopVertex {
    pub pos: [u16; 3],
    pub ao: u8,
    pub pad: u8,
    pub normal: [i8; 3],
    pub pad2: u8,
}

/// Wall, 20 bytes. `pos`: x and z over the block (or region) and its margin, y over the city's height range.
/// `uv`: `u` in the facade picture, `v / FACADE_V` in repeats of it. `color`: the wall's own colour, sRGB.
/// `gain`: brightness of the building's lit windows; `late`: how far into dusk they come on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct WallVertex {
    pub pos: [u16; 3],
    pub ao: u8,
    pub gain: u8,
    pub normal: [i8; 3],
    pub late: u8,
    pub color: [u8; 4],
    pub uv: [i16; 2],
}

/// `WallVertex::uv[1]` is stored divided by this, so tall walls fit an `i16`.
pub const FACADE_V: f32 = 32.0;

/// Painted geometry, 16 bytes. `color.a`: 255 for a lamp, which shines at night.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct SolidVertex {
    pub pos: [u16; 3],
    pub ao: u8,
    pub pad: u8,
    pub normal: [i8; 3],
    pub pad2: u8,
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TexHeader {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    /// `tex_format`.
    pub format: u32,
}

pub mod tex_format {
    /// BC1 blocks in row order.
    pub const BC1: u32 = 1;
    /// BC3 blocks in row order.
    pub const BC3: u32 = 3;

    /// Bytes of one level.
    pub fn level_bytes(format: u32, w: u32, h: u32) -> usize {
        let blocks = (w.max(4) as usize / 4) * (h.max(4) as usize / 4);
        blocks * if format == BC3 { 16 } else { 8 }
    }
}

pub fn bytes_of<T: Copy>(v: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}

pub fn slice_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { core::slice::from_raw_parts(v.as_ptr() as *const u8, core::mem::size_of_val(v)) }
}

pub fn read<T: Copy>(b: &[u8], at: usize) -> Option<T> {
    let n = core::mem::size_of::<T>();
    let s = b.get(at..at + n)?;
    Some(unsafe { core::ptr::read_unaligned(s.as_ptr() as *const T) })
}

/// Every record of a table section.
pub fn table<T: Copy>(b: &[u8]) -> Vec<T> {
    let n = core::mem::size_of::<T>();
    (0..b.len() / n).filter_map(|i| read::<T>(b, i * n)).collect()
}

/// The section table of a pack, from its first bytes.
pub struct Index {
    pub sections: Vec<(u32, usize, usize)>,
}

impl Index {
    /// Bytes of the header and the table of a pack with `count` sections.
    pub fn head_bytes(count: usize) -> usize {
        16 + count * 16
    }

    /// The section count, from the first 16 bytes.
    pub fn count(head: &[u8]) -> Result<usize, String> {
        let word = |i: usize| read::<u32>(head, i).ok_or_else(|| "pack is truncated".to_string());
        if word(0)? != MAGIC {
            return Err("not a pack".into());
        }
        if word(4)? != VERSION {
            return Err(format!("pack version {} (this build reads {VERSION})", word(4)?));
        }
        Ok(word(8)? as usize)
    }

    pub fn parse(bytes: &[u8]) -> Result<Index, String> {
        let n = Self::count(bytes)?;
        let mut sections = Vec::with_capacity(n);
        for i in 0..n {
            let at = 16 + i * 16;
            let word = |i: usize| read::<u32>(bytes, i).ok_or_else(|| "pack is truncated".to_string());
            sections.push((word(at)?, word(at + 4)? as usize, word(at + 8)? as usize));
        }
        Ok(Index { sections })
    }

    /// Offset and size of a section.
    pub fn range(&self, t: u32) -> Result<(usize, usize), String> {
        self.sections.iter().find(|s| s.0 == t).map(|s| (s.1, s.2)).ok_or_else(|| format!("pack has no {} section", String::from_utf8_lossy(&t.to_le_bytes())))
    }
}

/// Builds a pack in memory.
#[derive(Default)]
pub struct Writer {
    sections: Vec<(u32, Vec<u8>)>,
}

impl Writer {
    pub fn add(&mut self, t: u32, bytes: Vec<u8>) {
        self.sections.push((t, bytes));
    }

    pub fn finish(self) -> Vec<u8> {
        let pad = |n: usize| (n + 15) & !15;
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let mut at = pad(16 + self.sections.len() * 16);
        for (t, bytes) in &self.sections {
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&(at as u32).to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            at = pad(at + bytes.len());
        }
        for (_, bytes) in &self.sections {
            out.resize(pad(out.len()), 0);
            out.extend_from_slice(bytes);
        }
        out
    }
}
