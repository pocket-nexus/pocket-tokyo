//! Pictures for the machines without fragment programs. Light there is
//! `mix(day picture, night picture, t) × tint × mix(light, 1, t)`, so every
//! picture comes in two: as the day shows it, and as the night does (dark,
//! with lamp light on the ground and lit windows in the walls).
//!
//! The PSP takes a pair as one image of 8-bit indices with two palettes, which
//! the device mixes as the clock moves; the indices are clustered over both
//! colours of a texel at once. The 3DS takes two 16-bit pictures and mixes
//! them in its combiner.

use crate::target::Target;
use crate::tex::Image;
use std::collections::HashMap;

/// `img` at `w` × `h` (whole divisors of its size), each texel the mean of what it covers. Alpha rides along.
pub fn resize(img: &Image, w: usize, h: usize) -> Image {
    let (fx, fy) = (img.w / w, img.h / h);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            for j in 0..fy {
                for i in 0..fx {
                    let o = ((y * fy + j) * img.w + x * fx + i) * 4;
                    for c in 0..4 {
                        sum[c] += img.rgba[o + c] as u32;
                    }
                }
            }
            for c in 0..4 {
                rgba[(y * w + x) * 4 + c] = (sum[c] / (fx * fy) as u32) as u8;
            }
        }
    }
    Image { w, h, rgba }
}

/// The facade pictures for a handheld: the day picture with its walls at full brightness (a device multiplies
/// by the building's colour, and has no mask to tell wall from window), and the night picture: the same dimmed,
/// plus what the windows emit.
pub fn facades(day: &Image, night: &Image, w: usize, h: usize, wall_grey: f32) -> (Image, Image) {
    let (d, n) = (resize(day, w, h), resize(night, w, h));
    let mut day_out = d.rgba.clone();
    let mut night_out = vec![255u8; w * h * 4];
    for i in 0..w * h {
        let mask = d.rgba[i * 4 + 3] as f32 / 255.0;
        for c in 0..3 {
            let v = d.rgba[i * 4 + c] as f32;
            let lifted = (v + (v / wall_grey - v) * mask).min(255.0);
            day_out[i * 4 + c] = lifted as u8;
            night_out[i * 4 + c] = (lifted * [0.16, 0.18, 0.26][c] + n.rgba[i * 4 + c] as f32 * 2.0).min(255.0) as u8;
        }
        day_out[i * 4 + 3] = 255;
    }
    (Image { w, h, rgba: day_out }, Image { w, h, rgba: night_out })
}

/// A ground picture by night: its own colours under the night's ambient light and the lamps'. `lamp(u, v)`:
/// lamp light at a place of the picture (0..1 across and down).
pub fn night_ground(day: &Image, lamp: impl Fn(f32, f32) -> [f32; 3]) -> Image {
    let mut rgba = vec![255u8; day.w * day.h * 4];
    for y in 0..day.h {
        for x in 0..day.w {
            let l = lamp((x as f32 + 0.5) / day.w as f32, (y as f32 + 0.5) / day.h as f32);
            let o = (y * day.w + x) * 4;
            for c in 0..3 {
                rgba[o + c] = (day.rgba[o + c] as f32 * ([0.13, 0.15, 0.22][c] + l[c] * 0.8)).min(255.0) as u8;
            }
        }
    }
    Image { w: day.w, h: day.h, rgba }
}

fn psp_swizzle(src: &[u8], row_bytes: usize, rows: usize) -> Vec<u8> {
    let stride = (row_bytes + 15) & !15;
    let padded = (rows + 7) & !7;
    let mut out = vec![0u8; stride * padded];
    for y in 0..rows {
        for x in (0..row_bytes).step_by(16) {
            let n = (row_bytes - x).min(16);
            let dst = ((y / 8) * (stride / 16) + x / 16) * 128 + (y % 8) * 16;
            out[dst..dst + n].copy_from_slice(&src[y * row_bytes + x..y * row_bytes + x + n]);
        }
    }
    out
}

/// 16-bit texels into the PICA's layout: 8 × 8 tiles in row order, Morton order inside a tile, and the image's
/// last row first.
fn pica_tile(texels: &[u16], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 2];
    for y in 0..h {
        for x in 0..w {
            let fy = h - 1 - y;
            let tile = (fy / 8) * (w / 8) + x / 8;
            let (lx, ly) = (x % 8, fy % 8);
            let mut m = 0;
            for b in 0..3 {
                m |= ((lx >> b) & 1) << (2 * b);
                m |= ((ly >> b) & 1) << (2 * b + 1);
            }
            let o = (tile * 64 + m) * 2;
            out[o..o + 2].copy_from_slice(&texels[y * w + x].to_le_bytes());
        }
    }
    out
}

/// Day and night colour of a texel, five bits a channel, as one number.
fn key(d: &[u8], n: &[u8]) -> u32 {
    let q = |v: u8| (v >> 3) as u32;
    q(d[0]) | q(d[1]) << 5 | q(d[2]) << 10 | q(n[0]) << 15 | q(n[1]) << 20 | q(n[2]) << 25
}

fn channel(k: u32, c: usize) -> u32 {
    (k >> (5 * c)) & 31
}

/// At most `most` pairs of colours for a pair of pictures: median cut over the six channels of every texel's two
/// colours.
fn palette(day: &Image, night: &Image, most: usize) -> Vec<[u8; 6]> {
    let mut count: HashMap<u32, u32> = HashMap::new();
    for (d, n) in day.rgba.chunks_exact(4).zip(night.rgba.chunks_exact(4)) {
        *count.entry(key(d, n)).or_default() += 1;
    }
    let mut used: Vec<(u32, u32)> = count.into_iter().collect();
    used.sort_unstable();
    let mut boxes: Vec<Vec<(u32, u32)>> = vec![used];
    while boxes.len() < most {
        let Some(at) = (0..boxes.len()).filter(|&b| boxes[b].len() > 1).max_by_key(|&b| (boxes[b].iter().map(|e| e.1 as u64).sum::<u64>(), std::cmp::Reverse(b))) else { break };
        let mut b = boxes.swap_remove(at);
        let range = |c: usize| {
            let (lo, hi) = b.iter().fold((31, 0), |(lo, hi), e| (lo.min(channel(e.0, c)), hi.max(channel(e.0, c))));
            hi - lo
        };
        let c = (0..6).max_by_key(|&c| (range(c), std::cmp::Reverse(c))).unwrap();
        b.sort_by_key(|e| (channel(e.0, c), e.0));
        let total: u64 = b.iter().map(|e| e.1 as u64).sum();
        let mut acc = 0u64;
        let mut cut = 1;
        for (k, e) in b.iter().enumerate() {
            acc += e.1 as u64;
            if acc * 2 >= total {
                cut = (k + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let tail = b.split_off(cut);
        boxes.push(b);
        boxes.push(tail);
    }
    boxes
        .iter()
        .map(|b| {
            let n: u64 = b.iter().map(|e| e.1 as u64).sum::<u64>().max(1);
            core::array::from_fn(|c| (b.iter().map(|e| (channel(e.0, c) * 8 + 4) as u64 * e.1 as u64).sum::<u64>() / n) as u8)
        })
        .collect()
}

/// A pair of pictures as the PSP takes them: the day palette and the night palette (256 × 4 bytes each, red
/// first), then `levels` images of indices, largest first, each swizzled. With `colors` 128, every index is
/// below 128 and the device keeps the other half of the palette for itself: the same colours in shadow.
fn psp_pair(day: Image, night: Image, levels: usize, colors: usize) -> Vec<u8> {
    let colors = palette(&day, &night, colors);
    let mut out = Vec::new();
    for half in 0..2 {
        for k in 0..256 {
            let c = colors.get(k).copied().unwrap_or([0; 6]);
            out.extend_from_slice(&[c[half * 3], c[half * 3 + 1], c[half * 3 + 2], 255]);
        }
    }
    let mut nearest: HashMap<u32, u8> = HashMap::new();
    let (mut d, mut n) = (day, night);
    for level in 0..levels {
        let index: Vec<u8> = d
            .rgba
            .chunks_exact(4)
            .zip(n.rgba.chunks_exact(4))
            .map(|(a, b)| {
                *nearest.entry(key(a, b)).or_insert_with(|| {
                    let p = [a[0], a[1], a[2], b[0], b[1], b[2]].map(|v| v as i32);
                    (0..colors.len()).min_by_key(|&k| (0..6).map(|c| (colors[k][c] as i32 - p[c]).pow(2)).sum::<i32>()).unwrap_or(0) as u8
                })
            })
            .collect();
        out.extend_from_slice(&psp_swizzle(&index, d.w, d.h));
        if level + 1 < levels {
            d = d.half();
            n = n.half();
        }
    }
    out
}

/// A pair as the 3DS takes them: the day picture's levels, then the night picture's, 16-bit, tiled.
fn pica_pair(day: Image, night: Image, levels: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for mut img in [day, night] {
        for level in 0..levels {
            let texels: Vec<u16> = img.rgba.chunks_exact(4).map(|p| ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)).collect();
            out.extend_from_slice(&pica_tile(&texels, img.w, img.h));
            if level + 1 < levels {
                img = img.half();
            }
        }
    }
    out
}

/// Levels a picture of this size is stored at: down to 16 texels on its short side.
pub fn levels_of(w: usize, h: usize) -> usize {
    let mut n = 1;
    let mut s = w.min(h);
    while s > 16 {
        s /= 2;
        n += 1;
    }
    n
}

/// A pair with holes as the PSP takes it: 64 pairs of colours at four strengths of covering, so index `k` is
/// colour `k % 64` covered `k / 64` thirds.
fn psp_holes(day: Image, night: Image, levels: usize) -> Vec<u8> {
    // (the colours of what is covered at all)
    let covered = |img: &Image| Image { w: img.w, h: img.h, rgba: img.rgba.chunks_exact(4).zip(day.rgba.chunks_exact(4)).filter(|(_, d)| d[3] > 24).flat_map(|(p, _)| p.iter().copied()).collect() };
    let colors = palette(&covered(&day), &covered(&night), 64);
    let mut out = Vec::new();
    for half in 0..2 {
        for k in 0..256 {
            let c = colors.get(k % 64).copied().unwrap_or([0; 6]);
            out.extend_from_slice(&[c[half * 3], c[half * 3 + 1], c[half * 3 + 2], (k / 64 * 85) as u8]);
        }
    }
    let (mut d, mut n) = (day, night);
    for level in 0..levels {
        let index: Vec<u8> = d
            .rgba
            .chunks_exact(4)
            .zip(n.rgba.chunks_exact(4))
            .map(|(a, b)| {
                let p = [a[0], a[1], a[2], b[0], b[1], b[2]].map(|v| v as i32);
                let color = (0..colors.len()).min_by_key(|&k| (0..6).map(|c| (colors[k][c] as i32 - p[c]).pow(2)).sum::<i32>()).unwrap_or(0);
                (color + ((a[3] as usize + 42) / 85) * 64) as u8
            })
            .collect();
        out.extend_from_slice(&psp_swizzle(&index, d.w, d.h));
        if level + 1 < levels {
            d = d.half();
            n = n.half();
        }
    }
    out
}

/// A pair with holes as the 3DS takes it: the day picture's levels, then the night picture's, four bits a
/// channel with the covering last, tiled.
fn pica_holes(day: Image, night: Image, levels: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for mut img in [day, night] {
        for level in 0..levels {
            let texels: Vec<u16> = img.rgba.chunks_exact(4).map(|p| ((p[0] as u16 >> 4) << 12) | ((p[1] as u16 >> 4) << 8) | ((p[2] as u16 >> 4) << 4) | (p[3] as u16 >> 4)).collect();
            out.extend_from_slice(&pica_tile(&texels, img.w, img.h));
            if level + 1 < levels {
                img = img.half();
            }
        }
    }
    out
}

/// A day and night pair as the machine takes it, and how many levels it has: at most `most`. `colors`: of a
/// PSP's palette, how many the picture uses; 0 for a picture with holes.
pub fn pair(target: Target, day: Image, night: Image, most: usize, colors: usize) -> (Vec<u8>, usize) {
    let levels = levels_of(day.w, day.h).min(most);
    let bytes = match (target, colors) {
        (Target::Psp, 0) => psp_holes(day, night, levels),
        (Target::Psp, _) => psp_pair(day, night, levels, colors),
        (_, 0) => pica_holes(day, night, levels),
        _ => pica_pair(day, night, levels),
    };
    (bytes, levels)
}
