//! CityIR as the export page writes it (`web/src/pocket/export.js`).

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Named typed arrays and a JSON record (`web/src/pocket/bundle.js`).
pub struct Bundle {
    pub meta: Value,
    arrays: HashMap<String, (String, usize, usize)>,
    data: Vec<u8>,
}

impl Bundle {
    pub fn read(path: &Path) -> Result<Bundle, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() < 8 || &bytes[0..4] != b"CIR1" {
            return Err(format!("{}: not a CityIR bundle", path.display()));
        }
        let n = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let head: Value = serde_json::from_slice(&bytes[8..8 + n]).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut arrays = HashMap::new();
        if let Some(map) = head["arrays"].as_object() {
            for (name, a) in map {
                arrays.insert(name.clone(), (a["type"].as_str().unwrap_or("").to_string(), a["offset"].as_u64().unwrap_or(0) as usize, a["length"].as_u64().unwrap_or(0) as usize));
            }
        }
        Ok(Bundle { meta: head["meta"].clone(), arrays, data: bytes[8 + n..].to_vec() })
    }

    fn raw(&self, name: &str, kind: &str, size: usize) -> &[u8] {
        match self.arrays.get(name) {
            Some((t, offset, length)) => {
                assert_eq!(t, kind, "array {name} is a {t}");
                &self.data[*offset..*offset + *length * size]
            }
            None => &[],
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.arrays.contains_key(name)
    }

    pub fn f32(&self, name: &str) -> Vec<f32> {
        self.raw(name, "Float32Array", 4).chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
    }

    pub fn u32(&self, name: &str) -> Vec<u32> {
        self.raw(name, "Uint32Array", 4).chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect()
    }

    pub fn u8(&self, name: &str) -> Vec<u8> {
        self.raw(name, "Uint8Array", 1).to_vec()
    }
}

/// A height grid: rows run along +z, `data[j * w + i]` at `(x0 + i * step, z0 + j * step)`.
#[derive(Clone)]
pub struct Grid {
    pub x0: f32,
    pub z0: f32,
    pub step: f32,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Grid {
    /// Bilinear height, clamped to the grid's edges.
    pub fn sample(&self, x: f32, z: f32) -> f32 {
        let fx = ((x - self.x0) / self.step).clamp(0.0, self.w as f32 - 1.001);
        let fz = ((z - self.z0) / self.step).clamp(0.0, self.h as f32 - 1.001);
        let (i, j) = (fx as usize, fz as usize);
        let (tx, tz) = (fx - i as f32, fz - j as f32);
        let d = &self.data;
        (d[j * self.w + i] * (1.0 - tx) + d[j * self.w + i + 1] * tx) * (1.0 - tz) + (d[(j + 1) * self.w + i] * (1.0 - tx) + d[(j + 1) * self.w + i + 1] * tx) * tz
    }

    pub fn normal(&self, x: f32, z: f32) -> [f32; 3] {
        let e = self.step;
        let dx = (self.sample(x + e, z) - self.sample(x - e, z)) / (2.0 * e);
        let dz = (self.sample(x, z + e) - self.sample(x, z - e)) / (2.0 * e);
        let l = (dx * dx + 1.0 + dz * dz).sqrt();
        [-dx / l, 1.0 / l, -dz / l]
    }
}

pub struct TileRef {
    pub x: i32,
    pub z: i32,
}

pub struct Manifest {
    pub json: Value,
    pub tile: f32,
    pub tiles: Vec<TileRef>,
    /// The height of whatever one stands on: the terrain, and the decks of street bridges.
    pub surface: Grid,
}

pub fn manifest(dir: &Path) -> Result<Manifest, String> {
    let text = std::fs::read(dir.join("manifest.json")).map_err(|e| format!("manifest.json: {e}"))?;
    let json: Value = serde_json::from_slice(&text).map_err(|e| format!("manifest.json: {e}"))?;
    let t = &json["terrain"];
    let (w, h) = (t["w"].as_u64().unwrap_or(0) as usize, t["h"].as_u64().unwrap_or(0) as usize);
    let raw = std::fs::read(dir.join("surface.bin")).map_err(|e| format!("surface.bin: {e}"))?;
    if raw.len() != w * h * 4 {
        return Err("surface.bin does not match the terrain grid".into());
    }
    let data = raw.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let surface = Grid { x0: t["x0"].as_f64().unwrap_or(0.0) as f32, z0: t["z0"].as_f64().unwrap_or(0.0) as f32, step: t["step"].as_f64().unwrap_or(5.0) as f32, w, h, data };
    let tiles = json["tiles"].as_array().map(|a| a.iter().map(|t| TileRef { x: t["x"].as_i64().unwrap_or(0) as i32, z: t["z"].as_i64().unwrap_or(0) as i32 }).collect()).unwrap_or_default();
    Ok(Manifest { tile: json["tileSize"].as_f64().unwrap_or(256.0) as f32, tiles, surface, json })
}
