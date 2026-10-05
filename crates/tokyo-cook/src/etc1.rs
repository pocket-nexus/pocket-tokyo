//! ETC1, the PICA200's compressed texture format: 4 by 4 texels in 8 bytes.
//! A block is two halves (side by side, or one above the other), each a base
//! colour and one of eight tables of four brightness steps; every texel picks
//! a step. The encoder takes each half's mean as its base and the table that
//! leaves the least error, and keeps the better of the two ways to halve.

use crate::tex::Image;

const STEPS: [[i32; 4]; 8] = [[2, 8, -2, -8], [5, 17, -5, -17], [9, 29, -9, -29], [13, 42, -13, -42], [18, 60, -18, -60], [24, 80, -24, -80], [33, 106, -33, -106], [47, 183, -47, -183]];

/// The best table for texels `px` around `base`: (error, table, the step each texel picks).
fn half(px: &[[i32; 3]], base: [i32; 3]) -> (i64, usize, Vec<usize>) {
    let mut best = (i64::MAX, 0, Vec::new());
    for (t, steps) in STEPS.iter().enumerate() {
        let mut total = 0i64;
        let mut picks = Vec::with_capacity(px.len());
        for p in px {
            let (mut least, mut pick) = (i64::MAX, 0);
            for (k, s) in steps.iter().enumerate() {
                let e: i64 = (0..3).map(|c| ((base[c] + s).clamp(0, 255) - p[c]) as i64).map(|d| d * d).sum();
                if e < least {
                    (least, pick) = (e, k);
                }
            }
            total += least;
            picks.push(pick);
        }
        if total < best.0 {
            best = (total, t, picks);
        }
    }
    best
}

/// One block, `px[y * 4 + x]`, as the format's 8 bytes in its own (big-endian) order.
fn block(px: &[[i32; 3]; 16]) -> [u8; 8] {
    let mut best: (i64, [u8; 8]) = (i64::MAX, [0; 8]);
    for flip in 0..2 {
        // The texels of each half: left and right, or top and bottom.
        let of = |h: usize| -> Vec<usize> { (0..16).filter(|k| (if flip == 0 { k % 4 } else { k / 4 }) / 2 == h).collect() };
        let mean = |ids: &[usize]| -> [f32; 3] { [0, 1, 2].map(|c| ids.iter().map(|&k| px[k][c] as f32).sum::<f32>() / ids.len() as f32) };
        let (ids0, ids1) = (of(0), of(1));
        let (m0, m1) = (mean(&ids0), mean(&ids1));
        // Five bits a channel with the second base within -4..3 of the first, or four bits each.
        let q5 = |m: [f32; 3]| m.map(|v| (v * 31.0 / 255.0).round() as i32);
        let (a5, b5) = (q5(m0), q5(m1));
        let near = (0..3).all(|c| (-4..=3).contains(&(b5[c] - a5[c])));
        let (bases, head): ([[i32; 3]; 2], [u8; 3]) = if near {
            ([a5.map(|v| (v << 3) | (v >> 2)), b5.map(|v| (v << 3) | (v >> 2))], [0, 1, 2].map(|c| ((a5[c] << 3) | ((b5[c] - a5[c]) & 7)) as u8))
        } else {
            let q4 = |m: [f32; 3]| m.map(|v| (v * 15.0 / 255.0).round() as i32);
            let (a4, b4) = (q4(m0), q4(m1));
            ([a4.map(|v| (v << 4) | v), b4.map(|v| (v << 4) | v)], [0, 1, 2].map(|c| ((a4[c] << 4) | b4[c]) as u8))
        };
        let texels = |ids: &[usize]| -> Vec<[i32; 3]> { ids.iter().map(|&k| px[k]).collect() };
        let (h0, h1) = (half(&texels(&ids0), bases[0]), half(&texels(&ids1), bases[1]));
        if h0.0 + h1.0 >= best.0 {
            continue;
        }
        // A texel's step is two bits, kept in two words of sixteen: its place in them is x * 4 + y.
        let mut bits = 0u32;
        for (ids, picks) in [(&ids0, &h0.2), (&ids1, &h1.2)] {
            for (&k, &pick) in ids.iter().zip(picks) {
                let at = (k % 4) * 4 + k / 4;
                bits |= ((pick as u32 & 1) << at) | ((pick as u32 >> 1) << (16 + at));
            }
        }
        let control = ((h0.1 as u8) << 5) | ((h1.1 as u8) << 2) | ((near as u8) << 1) | flip as u8;
        let b = bits.to_be_bytes();
        best = (h0.0 + h1.0, [head[0], head[1], head[2], control, b[0], b[1], b[2], b[3]]);
    }
    best.1
}

/// A picture as the PICA200 stores an ETC1 texture: the last row first, squares of 8 by 8 texels in rows, the
/// four blocks of a square in a Z, each block's bytes last to first.
pub fn pica(img: &Image) -> Vec<u8> {
    let (w, h) = (img.w, img.h);
    let mut out = Vec::with_capacity(w * h / 2);
    for ty in 0..h / 8 {
        for tx in 0..w / 8 {
            for sub in 0..4 {
                let (bx, by) = (tx * 8 + (sub & 1) * 4, ty * 8 + (sub >> 1) * 4);
                let mut px = [[0i32; 3]; 16];
                for y in 0..4 {
                    for x in 0..4 {
                        let o = ((h - 1 - (by + y)) * w + bx + x) * 4;
                        px[y * 4 + x] = [img.rgba[o] as i32, img.rgba[o + 1] as i32, img.rgba[o + 2] as i32];
                    }
                }
                out.extend(block(&px).iter().rev());
            }
        }
    }
    out
}

/// What a block decodes to, for the test: `out[y * 4 + x]`.
#[cfg(test)]
fn decode(b: &[u8; 8]) -> [[i32; 3]; 16] {
    let (flip, diff) = (b[3] & 1, b[3] & 2 != 0);
    let tables = [(b[3] >> 5) as usize, ((b[3] >> 2) & 7) as usize];
    let bases: [[i32; 3]; 2] = if diff {
        let a = [0, 1, 2].map(|c| (b[c] >> 3) as i32);
        let d = [0, 1, 2].map(|c| (((b[c] & 7) as i32) << 29) >> 29);
        [a.map(|v| (v << 3) | (v >> 2)), [0, 1, 2].map(|c| a[c] + d[c]).map(|v| (v << 3) | (v >> 2))]
    } else {
        [[0, 1, 2].map(|c| (b[c] >> 4) as i32).map(|v| (v << 4) | v), [0, 1, 2].map(|c| (b[c] & 15) as i32).map(|v| (v << 4) | v)]
    };
    let bits = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    core::array::from_fn(|k| {
        let (x, y) = (k % 4, k / 4);
        let h = (if flip == 0 { x } else { y }) / 2;
        let at = x * 4 + y;
        let pick = ((bits >> at) & 1) | (((bits >> (16 + at)) & 1) << 1);
        bases[h].map(|v| (v + STEPS[tables[h]][pick as usize]).clamp(0, 255))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_comes_back_close_to_what_went_in() {
        // Two flat halves, one above the other: the encoder must halve the block that way and land on both.
        let px: [[i32; 3]; 16] = core::array::from_fn(|k| if k / 4 < 2 { [200, 40, 40] } else { [30, 90, 160] });
        let got = decode(&block(&px));
        for k in 0..16 {
            for c in 0..3 {
                assert!((got[k][c] - px[k][c]).abs() <= 8, "texel {k} channel {c}: {} for {}", got[k][c], px[k][c]);
            }
        }
        // A ramp of greys: within the largest step of the table that fits.
        let ramp: [[i32; 3]; 16] = core::array::from_fn(|k| [60 + k as i32 * 6; 3]);
        let got = decode(&block(&ramp));
        assert!((0..16).all(|k| (got[k][0] - ramp[k][0]).abs() <= 14));
    }
}
