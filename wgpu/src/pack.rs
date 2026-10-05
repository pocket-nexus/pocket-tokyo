//! The pack, read as a handheld reads it: its tables and the levels that stay
//! in memory at the start, and a cell's near level (one record of `NEAR`) when
//! the eye comes near. The file is an iPod touch pack as the city compiler
//! writes it (`profiles/ipod60.json`): texels of 16 bits in row order.
//!
//! A handheld has the whole file beside it and reads the blocks' pictures of
//! the ground with the head. A tab does not wait for them: the head is the
//! tables, the levels' vertices and the facades (9.3 MB of the 42.9 MB before
//! `NEAR`), and a block's picture is a read of its own afterwards
//! ([`Tables::picture`]), the nearest block first.

use pocket_web_wgpu::picture;
use pocket_web_wgpu::source::Source;
use tokyo_pack::{self as pack, table, Batch, Block, Cell, City, HandPicture, Index, Landmark, NearCell, Region};


/// Side of the textures over the grid of heights: the lamps' light and the shadows.
pub const MAP_SIDE: u32 = 1024;
/// The most sections a pack's table is read for.
const SECTIONS: usize = 32;

/// The tables the core keeps for the life of the program.
pub struct Tables {
    pub city: City,
    pub regions: Vec<Region>,
    pub blocks: Vec<Block>,
    pub cells: Vec<Cell>,
    pub batches: Vec<Batch>,
    pub spans: Vec<u32>,
    pub tour: Vec<[f32; 6]>,
    pub heights: Vec<u16>,
    pub near: Vec<NearCell>,
    pub landmarks: Vec<Landmark>,
    pub meta: Vec<u8>,
    /// Where `NEAR` starts in the file.
    pub near_offset: u32,
    /// The largest record of `NEAR`.
    pub slot_bytes: u32,
    /// One picture per block, then the facades, and where their texels (`HTEX`) start in the file.
    pub pictures: Vec<HandPicture>,
    pub texels_offset: u32,
}

/// What the GPU is handed at the start, in the bytes that were read: they may go once it has them.
pub struct Stays<'a> {
    /// Top, wall and solid vertices of the levels that stay in memory.
    pub vertices: [&'a [u8]; 3],
    pub indices: &'a [u8],
    /// The facades by day, then what their windows emit: two pictures of one size, one after the other.
    pub facades: &'a [u8],
    /// The lamps' light, `MAP_SIDE` squared texels of 16 bits.
    pub lamps: &'a [u8],
}

/// Sections of a pack, each read with as few requests as their places in the file allow.
pub struct Sections {
    index: Index,
    /// Ranges read: where each starts, and its bytes.
    held: Vec<(usize, Vec<u8>)>,
    facades: Vec<u8>,
}

impl Sections {
    async fn read(source: &Source, index: Index, tags: &[u32]) -> Result<Sections, String> {
        let mut wanted: Vec<(usize, usize)> = tags.iter().filter_map(|&t| index.range(t).ok()).filter(|r| r.1 > 0).collect();
        wanted.sort();
        // Sections that follow one another in the file are one request.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (at, size) in wanted {
            match runs.last_mut() {
                Some(run) if at <= run.0 + run.1 + 4096 => run.1 = at + size - run.0,
                _ => runs.push((at, size)),
            }
        }
        let mut held = Vec::with_capacity(runs.len());
        for (at, size) in runs {
            held.push((at, source.range(at as u64, size as u64).await?));
        }
        Ok(Sections { index, held, facades: Vec::new() })
    }

    fn get(&self, tag: u32) -> Result<&[u8], String> {
        let (at, size) = self.index.range(tag)?;
        if size == 0 {
            return Ok(&[]);
        }
        self.held.iter().find(|h| at >= h.0 && at + size <= h.0 + h.1.len()).map(|h| &h.1[at - h.0..at - h.0 + size]).ok_or_else(|| "a section of the pack was not read".into())
    }
}

/// Reads the head of a pack: the tables, and the sections the GPU draws from before any cell has arrived
/// (`stays`).
pub async fn open(source: &Source) -> Result<(Tables, Sections), String> {
    let head = source.range(0, Index::head_bytes(SECTIONS) as u64).await?;
    let count = Index::count(&head)?;
    if count > SECTIONS {
        return Err(format!("the pack has {count} sections"));
    }
    let index = Index::parse(&head)?;
    let near_offset = index.range(pack::NEAR)?.0 as u32;
    let wanted = [
        pack::META, pack::CITY, pack::REGN, pack::BLCK, pack::CELL, pack::BTCH, pack::SPAN, pack::VTOP, pack::VWAL, pack::VSOL, pack::IDX0, pack::HMAP, pack::LAMP,
        pack::HPIC, pack::NCEL, pack::LAND, pack::TOUR,
    ];
    let texels = index.range(pack::HTEX)?;
    let mut s = Sections::read(source, index, &wanted).await?;
    let city: City = pack::read(s.get(pack::CITY)?, 0).ok_or("the pack's CITY is cut short")?;
    let lamps = s.get(pack::LAMP)?;
    if lamps.len() != (MAP_SIDE * MAP_SIDE * 2) as usize || city.grid_w > MAP_SIDE || city.grid_h > MAP_SIDE {
        return Err("the pack is not an iPod touch pack: cook with --profile ipod60".into());
    }
    let heights: Vec<u16> = table(s.get(pack::HMAP)?);
    if heights.len() < (city.grid_w * city.grid_h) as usize {
        return Err("the pack's heights are cut short".into());
    }
    let near: Vec<NearCell> = table(s.get(pack::NCEL)?);
    let pictures: Vec<HandPicture> = table(s.get(pack::HPIC)?);
    let tables = Tables {
        city,
        regions: table(s.get(pack::REGN)?),
        blocks: table(s.get(pack::BLCK)?),
        cells: table(s.get(pack::CELL)?),
        batches: table(s.get(pack::BTCH)?),
        spans: table(s.get(pack::SPAN)?),
        tour: table(s.get(pack::TOUR)?),
        heights,
        slot_bytes: near.iter().map(|n| n.size).max().unwrap_or(0),
        near,
        // (a pack without a landmark has no table of them)
        landmarks: s.get(pack::LAND).map(table).unwrap_or_default(),
        meta: s.get(pack::META)?.to_vec(),
        near_offset,
        pictures,
        texels_offset: texels.0 as u32,
    };
    if tables.near.len() != tables.cells.len() {
        return Err("the pack's cells and their records do not match".into());
    }
    if tables.pictures.len() != tables.blocks.len() + 1 {
        return Err("the pack's pictures are not one per block and the facades".into());
    }
    // The facades stand on every wall from the first frame: they are read with the head.
    let f = tables.pictures[tables.blocks.len()];
    let pair = 2 * picture::stored_bytes(f.width as u32, f.height as u32, f.levels) as u64;
    if f.offset as u64 + pair > texels.1 as u64 {
        return Err("the pack's facades are cut short".into());
    }
    s.facades = source.range(texels.0 as u64 + f.offset as u64, pair).await?;
    Ok((tables, s))
}

impl Sections {
    pub fn stays(&self) -> Result<Stays<'_>, String> {
        Ok(Stays {
            vertices: [self.get(pack::VTOP)?, self.get(pack::VWAL)?, self.get(pack::VSOL)?],
            indices: self.get(pack::IDX0)?,
            facades: &self.facades,
            lamps: self.get(pack::LAMP)?,
        })
    }
}

/// Where part `k` of a cell's record starts: each part begins on a 16-byte boundary.
pub fn part(record: &NearCell, k: usize) -> usize {
    record.parts[..k].iter().map(|&p| (p as usize + 15) & !15).sum()
}

impl Tables {
    /// Where a block's picture of the ground is in the file, and its bytes.
    pub fn picture(&self, block: usize) -> (u64, u64) {
        let p = &self.pictures[block];
        (self.texels_offset as u64 + p.offset as u64, picture::stored_bytes(p.width as u32, p.height as u32, p.levels) as u64)
    }

    /// The middle of a block on the ground.
    pub fn middle(&self, block: usize) -> (f32, f32) {
        let c = &self.city;
        (c.x0 + ((block as u32 % c.blocks_x) as f32 + 0.5) * c.block, c.z0 + ((block as u32 / c.blocks_x) as f32 + 0.5) * c.block)
    }

    /// The tables as the core takes them. They stay where they are: `self` lives as long as the program.
    pub fn for_core(&'static self) -> tokyo_core::Pack {
        tokyo_core::Pack {
            city: &self.city,
            regions: self.regions.as_ptr(),
            region_count: self.regions.len() as u32,
            blocks: self.blocks.as_ptr(),
            block_count: self.blocks.len() as u32,
            cells: self.cells.as_ptr(),
            cell_count: self.cells.len() as u32,
            batches: self.batches.as_ptr(),
            batch_count: self.batches.len() as u32,
            spans: self.spans.as_ptr(),
            span_count: self.spans.len() as u32,
            tour: self.tour.as_ptr(),
            tour_count: self.tour.len() as u32,
            heights: self.heights.as_ptr(),
            near: self.near.as_ptr(),
            near_offset: self.near_offset,
            meta: self.meta.as_ptr(),
            meta_len: self.meta.len() as u32,
            landmarks: self.landmarks.as_ptr(),
            landmark_count: self.landmarks.len() as u32,
        }
    }
}
