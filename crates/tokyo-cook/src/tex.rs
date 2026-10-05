//! Pictures for the device: mip chains averaged in linear light, then block compression. Compressed levels are
//! kept under `.pocket-build/cache/bc`, named by what went in.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokyo_pack::{tex_format, TexHeader};

pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

pub fn load(path: &Path) -> Result<Image, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
    Ok(Image { w: img.width() as usize, h: img.height() as usize, rgba: img.into_raw() })
}

fn tables() -> &'static ([f32; 256], Vec<u8>) {
    static T: OnceLock<([f32; 256], Vec<u8>)> = OnceLock::new();
    T.get_or_init(|| {
        let mut lin = [0.0f32; 256];
        for (i, l) in lin.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *l = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        let srgb = (0..4096).map(|i| crate::geom::srgb8(i as f32 / 4095.0)).collect();
        (lin, srgb)
    })
}

impl Image {
    /// One colour all over.
    pub fn blank(w: usize, h: usize, rgb: [u8; 3]) -> Image {
        Image { w, h, rgba: [rgb[0], rgb[1], rgb[2], 255].repeat(w * h) }
    }

    /// Copies `src` in with its corner at `(x, y)`.
    pub fn blit(&mut self, src: &Image, x: usize, y: usize) {
        for row in 0..src.h.min(self.h.saturating_sub(y)) {
            let n = src.w.min(self.w.saturating_sub(x)) * 4;
            let to = ((y + row) * self.w + x) * 4;
            self.rgba[to..to + n].copy_from_slice(&src.rgba[row * src.w * 4..row * src.w * 4 + n]);
        }
    }

    /// Half the size: each texel the mean of four, colour in linear light.
    pub fn half(&self) -> Image {
        let (lin, srgb) = tables();
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let o = ((y * 2 + dy).min(self.h - 1) * self.w + (x * 2 + dx).min(self.w - 1)) * 4;
                    for c in 0..3 {
                        sum[c] += lin[self.rgba[o + c] as usize];
                    }
                    sum[3] += self.rgba[o + 3] as f32;
                }
                let o = (y * w + x) * 4;
                for c in 0..3 {
                    rgba[o + c] = srgb[((sum[c] / 4.0) * 4095.0 + 0.5) as usize];
                }
                rgba[o + 3] = (sum[3] / 4.0 + 0.5) as u8;
            }
        }
        Image { w, h, rgba }
    }

    /// Mean colour, sRGB.
    pub fn mean(&self) -> [u8; 3] {
        let (lin, srgb) = tables();
        let mut sum = [0.0f64; 3];
        for px in self.rgba.chunks_exact(4) {
            for c in 0..3 {
                sum[c] += lin[px[c] as usize] as f64;
            }
        }
        let n = (self.w * self.h) as f64;
        [0, 1, 2].map(|c| srgb[((sum[c] / n) * 4095.0 + 0.5) as usize])
    }
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Cache {
        let _ = std::fs::create_dir_all(&dir);
        Cache { dir }
    }

    /// One level, block-compressed.
    pub fn compress(&self, img: &Image, format: u32) -> Vec<u8> {
        let mut h = Sha256::new();
        h.update([format as u8, 1]);
        h.update((img.w as u32).to_le_bytes());
        h.update(&img.rgba);
        let name: String = h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect();
        let path = self.dir.join(name);
        let size = tex_format::level_bytes(format, img.w as u32, img.h as u32);
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.len() == size {
                return bytes;
            }
        }
        let fmt = if format == tex_format::BC3 { texpresso::Format::Bc3 } else { texpresso::Format::Bc1 };
        let mut out = vec![0u8; fmt.compressed_size(img.w, img.h)];
        let params = texpresso::Params { algorithm: texpresso::Algorithm::ClusterFit, ..Default::default() };
        fmt.compress(&img.rgba, img.w, img.h, params, &mut out);
        let _ = std::fs::write(&path, &out);
        out
    }

    /// `TexHeader` and `mips` levels, largest first.
    pub fn chain(&self, img: Image, format: u32, mips: u32) -> Vec<u8> {
        let head = TexHeader { width: img.w as u32, height: img.h as u32, mips, format };
        let mut bytes = tokyo_pack::bytes_of(&head).to_vec();
        let mut level = img;
        for i in 0..mips {
            bytes.extend_from_slice(&self.compress(&level, format));
            if i + 1 < mips {
                level = level.half();
            }
        }
        bytes
    }
}
