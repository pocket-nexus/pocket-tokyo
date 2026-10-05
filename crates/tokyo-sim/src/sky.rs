//! The clock and what it does to the light: where the sun and the moon
//! stand over Tokyo at an hour of the day, and the colours of the sun, the
//! sky and the haze that follow from it.

use crate::math::*;

/// Tokyo Tower: 35.6586° N.
const LATITUDE: f32 = 0.622_36;

/// Direction to the sun at `hour` (0..24, local solar time) on day `day` of the year (0 = 1 January).
/// The frame is the city's: x east, y up, z south.
pub fn sun_direction(hour: f32, day: f32) -> V3 {
    let decl = -0.409_28 * cos(TAU * (day + 10.0) / 365.0);
    let h = (hour - 12.0) * (PI / 12.0);
    let (sl, cl) = (sin(LATITUDE), cos(LATITUDE));
    let (sd, cd) = (sin(decl), cos(decl));
    // East, north and up from the hour angle.
    let east = -cd * sin(h);
    let north = cl * sd - sl * cd * cos(h);
    let up = sl * sd + cl * cd * cos(h);
    v3(east, up, -north)
}

/// The light of an hour, colours in the display's own encoding (sRGB values, used as they are).
#[derive(Clone, Copy, Debug, Default)]
pub struct Light {
    /// Towards the light that casts shadows: the sun by day, the moon at night.
    pub dir: V3,
    pub sun: [f32; 3],
    /// Light from the sky on a face that looks up, and on one that looks down.
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    /// The haze towards the horizon and overhead.
    pub horizon: [f32; 3],
    pub zenith: [f32; 3],
    /// 0 by day, 1 when the city's lights are fully on.
    pub night: f32,
    /// 0 by day, 1 when the sky is dark.
    pub dark: f32,
    /// Where the sun itself is, for the sky.
    pub sun_dir: V3,
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

/// Keys along the sun's height (sine of its elevation): colours of the sun, the sky light, the horizon and the zenith.
const KEYS: [(f32, [f32; 3], [f32; 3], [f32; 3], [f32; 3]); 6] = [
    (-0.30, [0.10, 0.13, 0.22], [0.10, 0.12, 0.20], [0.10, 0.09, 0.12], [0.015, 0.02, 0.05]),
    (-0.10, [0.12, 0.14, 0.24], [0.16, 0.17, 0.27], [0.36, 0.24, 0.26], [0.05, 0.07, 0.17]),
    (0.00, [0.95, 0.42, 0.16], [0.34, 0.32, 0.40], [0.95, 0.55, 0.30], [0.16, 0.22, 0.42]),
    (0.10, [1.00, 0.68, 0.40], [0.42, 0.44, 0.52], [0.92, 0.74, 0.58], [0.22, 0.36, 0.62]),
    (0.30, [1.00, 0.90, 0.76], [0.46, 0.52, 0.62], [0.76, 0.82, 0.90], [0.20, 0.40, 0.72]),
    (1.00, [1.00, 0.96, 0.88], [0.48, 0.55, 0.66], [0.72, 0.80, 0.90], [0.17, 0.36, 0.70]),
];

pub fn light(hour: f32, day: f32) -> Light {
    let sun_dir = sun_direction(hour, day);
    let h = sun_dir.y;
    let mut i = 0;
    while i + 2 < KEYS.len() && h > KEYS[i + 1].0 {
        i += 1;
    }
    let (a, b) = (&KEYS[i], &KEYS[i + 1]);
    let t = saturate((h - a.0) / (b.0 - a.0));
    let night = smoothstep(0.12, -0.08, h);
    let dark = smoothstep(0.02, -0.22, h);
    // Below the horizon the sun gives way to the moon, which stands opposite it and higher than it is low.
    let moon = v3(-sun_dir.x, max(-sun_dir.y, 0.35), -sun_dir.z).norm();
    let by_sun = smoothstep(-0.06, 0.02, h);
    let dir = if by_sun > 0.0 { v3(sun_dir.x, max(sun_dir.y, 0.03), sun_dir.z).norm() } else { moon };
    let fade = if by_sun > 0.0 { by_sun } else { smoothstep(-0.06, -0.16, h) * 0.9 };
    let sun = mix3(a.1, b.1, t);
    let sky = mix3(a.2, b.2, t);
    Light {
        dir,
        sun: [sun[0] * fade, sun[1] * fade, sun[2] * fade],
        sky,
        ground: [sky[0] * 0.55, sky[1] * 0.52, sky[2] * 0.5],
        horizon: mix3(a.3, b.3, t),
        zenith: mix3(a.4, b.4, t),
        night,
        dark,
        sun_dir,
    }
}
