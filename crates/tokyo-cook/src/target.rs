//! What differs between machines in the pack: the bytes of a vertex.

/// The machine a pack is compiled for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// PS Vita: `tokyo_pack::{TopVertex, WallVertex, SolidVertex}`.
    Vita,
    /// PSP, as the GE reads vertices: texture coordinates, colour, normal, position.
    Psp,
    /// Nintendo 3DS, as the vertex programs read them.
    Pica,
}

impl Target {
    pub fn of(name: &str) -> Result<Target, String> {
        match name {
            "vita" => Ok(Target::Vita),
            "psp" => Ok(Target::Psp),
            "n3ds" => Ok(Target::Pica),
            other => Err(format!("target {other:?} is not one of vita, psp, n3ds")),
        }
    }

    /// Bytes of a top, a wall and a solid vertex.
    pub fn sizes(self) -> [usize; 3] {
        match self {
            Target::Vita => [12, 20, 16],
            Target::Psp => [8, 12, 8],
            Target::Pica => [12, 20, 16],
        }
    }
}

/// A vertex as its bytes, so one path welds and stores every machine's.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Raw {
    pub bytes: [u8; 20],
    pub len: u8,
}

impl Raw {
    pub fn of<T: Copy>(v: &T) -> Raw {
        let b = tokyo_pack::bytes_of(v);
        let mut bytes = [0u8; 20];
        bytes[..b.len()].copy_from_slice(b);
        Raw { bytes, len: b.len() as u8 }
    }

    pub fn from(parts: &[&[u8]]) -> Raw {
        let mut bytes = [0u8; 20];
        let mut len = 0;
        for p in parts {
            bytes[len..len + p.len()].copy_from_slice(p);
            len += p.len();
        }
        Raw { bytes, len: len as u8 }
    }
}

/// Appends vertices to a section and says which vertex of the section the first of them is.
pub fn store(to: &mut Vec<u8>, vertices: &[Raw]) -> usize {
    let size = vertices.first().map(|v| v.len as usize).unwrap_or(1);
    let first = to.len() / size;
    for v in vertices {
        to.extend_from_slice(&v.bytes[..v.len as usize]);
    }
    first
}

/// `r | g << 5 | b << 11`, as the GE reads a 16-bit colour.
pub fn psp565(rgb: [u8; 3]) -> [u8; 2] {
    (((rgb[0] as u16) >> 3) | (((rgb[1] as u16) >> 2) << 5) | (((rgb[2] as u16) >> 3) << 11)).to_le_bytes()
}
