//! The sky, and the glow of what is bright.
//!
//! The scene is drawn straight into the display's multisampled surface. At
//! night a quarter-size chain takes the frame the display is showing (the one
//! before this), keeps what is bright in it and blurs it, and the result is
//! added over this frame: lights glow one frame late, and no pass copies the
//! scene. Every tap's texture coordinate comes from the vertex stage, so no
//! fragment program computes one.
//!
//! For measurements the scene can go to an off-screen target of its own
//! instead, and be copied to the display: the GPU's time for that target does
//! not wait on the display's refresh.

use pocket_vita_gxm::mem::{Arena, Block, Kind};
use pocket_vita_gxm::program::F32;
use pocket_vita_gxm::target::{ColorFormat, Depth, Msaa, Target};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Gpu, Param, Program, Stream, Uniforms};

const POST_V: &str = include_str!("../shaders/post_v.cg");
const BRIGHT_F: &str = include_str!("../shaders/bright_f.cg");
const BLUR_F: &str = include_str!("../shaders/blur_f.cg");
const COMPOSITE_F: &str = include_str!("../shaders/composite_f.cg");
const GLOW_F: &str = include_str!("../shaders/glow_f.cg");
const SKY_V: &str = include_str!("../shaders/sky_v.cg");
const SKY_F: &str = include_str!("../shaders/sky_f.cg");

pub const W: u32 = 960;
pub const H: u32 = 544;
/// The bright chain runs at a quarter of the frame.
const QW: u32 = 240;
const QH: u32 = 136;

#[derive(Clone, Copy)]
pub struct Look {
    pub bloom: bool,
    /// Brightness above which a texel glows, and how strongly the glow is added.
    pub threshold: f32,
    pub bloom_gain: f32,
}

impl Look {
    pub const DEFAULT: Look = Look { bloom: true, threshold: 0.7, bloom_gain: 0.8 };
}

struct Pass {
    prog: Program,
    tap: Param,
    params: [Param; 2],
    samplers: [Option<u32>; 2],
}

impl Pass {
    unsafe fn new(gpu: &mut Gpu, name: &str, defines: &str, fs: &str, uniforms: [&str; 2], samplers: [&str; 2], msaa: u32, blend: Blend) -> Result<Pass, String> {
        let prog = gpu.program(name, defines, POST_V, fs, &[Stream { stride: 8, instanced: false, attrs: &[("aPosition", 0, F32, 2)] }], msaa, [blend, Blend::Alpha])?;
        let tap = prog.vs.param("uTap");
        let params = uniforms.map(|u| if u.is_empty() { core::ptr::null() } else { prog.fs.param(u) });
        let samplers = samplers.map(|s| if s.is_empty() { None } else { prog.fs.sampler_index(s) });
        Ok(Pass { prog, tap, params, samplers })
    }

    /// Draws the full-screen triangle with this pass's taps, fragment uniforms and textures.
    unsafe fn draw(&self, ctx: *mut g::SceGxmContext, tri: (*const u8, *const u16), taps: &[f32; 32], uniforms: [&[f32]; 2], textures: [Option<&g::SceGxmTexture>; 2]) {
        self.prog.bind(ctx, false);
        gpu::state_overlay(ctx, false);
        Uniforms::vertex(ctx).set(self.tap, taps);
        if self.params.iter().any(|p| !p.is_null()) {
            let f = Uniforms::fragment(ctx);
            for (p, v) in self.params.iter().zip(uniforms) {
                if !v.is_empty() {
                    f.set(*p, v);
                }
            }
        }
        for (unit, tex) in self.samplers.iter().zip(textures) {
            if let (Some(unit), Some(tex)) = (unit, tex) {
                g::sceGxmSetFragmentTexture(ctx, *unit, tex);
            }
        }
        gpu::draw(ctx, tri.0, tri.1, 3);
    }
}

/// Eight identity taps.
fn taps() -> [f32; 32] {
    let mut t = [0.0; 32];
    for k in 0..8 {
        t[k * 4] = 1.0;
        t[k * 4 + 1] = 1.0;
    }
    t
}

pub struct Post {
    _block: Block,
    tri: (*const u8, *const u16),
    /// The scene's own target, for measurements.
    pub scene: Target,
    a: Target,
    b: Target,
    bright: Pass,
    blur: Pass,
    copy: Pass,
    glow: Pass,
    sky: Program,
    sky_mvp: Param,
    sky_look: Param,
    dome: (*const u8, *const u16, u32),
    /// Whether `a` holds a glow to add to this frame.
    lit: bool,
}

unsafe fn viewport(ctx: *mut g::SceGxmContext, w: u32, h: u32) {
    let (hw, hh) = (w as f32 * 0.5, h as f32 * 0.5);
    g::sceGxmSetViewport(ctx, hw, hw, hh, -hh, 0.0, 1.0);
    g::sceGxmSetRegionClip(ctx, g::SceGxmRegionClipMode_SCE_GXM_REGION_CLIP_OUTSIDE, 0, 0, w - 1, h - 1);
}

impl Post {
    /// # Safety
    /// GXM is initialized; the arenas outlive the targets.
    pub unsafe fn new(gpu: &mut Gpu, vram: &mut Arena, main: &mut Arena, msaa: Msaa) -> Result<Post, String> {
        let mut block = Block::with_access(Kind::Main, 16 * 1024, false)?;
        let vb = block.alloc(24, 16).ok_or("post triangle")?.cast::<f32>();
        let ib = block.alloc(8, 16).ok_or("post triangle")?.cast::<u16>();
        for (i, v) in [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0].into_iter().enumerate() {
            *vb.add(i) = v;
        }
        for i in 0..3 {
            *ib.add(i) = i as u16;
        }
        // The sky's dome: a unit sphere, rings closer together towards the horizon, where the colour changes fastest.
        const RINGS: usize = 14;
        const AROUND: usize = 24;
        let dv = block.alloc((RINGS + 1) * AROUND * 12, 16).ok_or("sky dome")?.cast::<f32>();
        let di = block.alloc(RINGS * AROUND * 12, 16).ok_or("sky dome")?.cast::<u16>();
        for r in 0..=RINGS {
            let t = r as f32 / RINGS as f32 * 2.0 - 1.0;
            let elevation = t * t * t.signum() * core::f32::consts::FRAC_PI_2;
            for a in 0..AROUND {
                let turn = a as f32 / AROUND as f32 * core::f32::consts::TAU;
                let at = dv.add((r * AROUND + a) * 3);
                *at = libm::cosf(elevation) * libm::cosf(turn);
                *at.add(1) = libm::sinf(elevation);
                *at.add(2) = libm::cosf(elevation) * libm::sinf(turn);
            }
        }
        for r in 0..RINGS {
            for a in 0..AROUND {
                let (p, q) = ((r * AROUND + a) as u16, (r * AROUND + (a + 1) % AROUND) as u16);
                let at = di.add((r * AROUND + a) * 6);
                for (k, v) in [p, q, p + AROUND as u16, q, q + AROUND as u16, p + AROUND as u16].into_iter().enumerate() {
                    *at.add(k) = v;
                }
            }
        }
        let scene = Target::new(vram, main, W, H, ColorFormat::Rgba8, msaa, Depth::Transient)?;
        let a = Target::new(vram, main, QW, QH, ColorFormat::Rgba8, Msaa::None, Depth::None)?;
        let b = Target::new(vram, main, QW, QH, ColorFormat::Rgba8, Msaa::None, Depth::None)?;
        let sky = gpu.program("sky", "", SKY_V, SKY_F, &[Stream { stride: 12, instanced: false, attrs: &[("aPosition", 0, F32, 3)] }], msaa.gxm(), [Blend::Opaque, Blend::Alpha])?;
        Ok(Post {
            tri: (vb.cast(), ib),
            _block: block,
            scene,
            a,
            b,
            bright: Pass::new(gpu, "bright", "", BRIGHT_F, ["uParams", ""], ["uSource", ""], 0, Blend::Opaque)?,
            blur: Pass::new(gpu, "blur", "", BLUR_F, ["", ""], ["uSource", ""], 0, Blend::Opaque)?,
            copy: Pass::new(gpu, "copy", "", COMPOSITE_F, ["", ""], ["uScene", ""], msaa.gxm(), Blend::Opaque)?,
            glow: Pass::new(gpu, "glow", "", GLOW_F, ["uGlow", ""], ["uBloom", ""], msaa.gxm(), Blend::Additive)?,
            sky_mvp: sky.vs.param("uMvp"),
            sky_look: sky.vs.param("uSky"),
            dome: (dv.cast(), di, (RINGS * AROUND * 6) as u32),
            sky,
            lit: false,
        })
    }

    /// Opens the scene's own target. Depth starts at the far plane.
    pub unsafe fn begin_scene(&mut self, ctx: *mut g::SceGxmContext) -> Result<(), String> {
        self.scene.begin(ctx, 1.0)?;
        viewport(ctx, W, H);
        Ok(())
    }

    /// The whole display, for a scene drawn straight into it.
    pub unsafe fn full_view(&self, ctx: *mut g::SceGxmContext) {
        viewport(ctx, W, H);
    }

    /// The sky behind everything. `mvp`: the view and projection with the eye at the origin.
    pub unsafe fn draw_sky(&self, ctx: *mut g::SceGxmContext, mvp: &[f32; 16], sky: &[f32; 16]) {
        self.sky.bind(ctx, false);
        gpu::state_overlay(ctx, true);
        let v = Uniforms::vertex(ctx);
        v.set(self.sky_mvp, mvp);
        v.set(self.sky_look, sky);
        gpu::draw(ctx, self.dome.0, self.dome.1, self.dome.2);
    }

    /// Closes the scene's own target. `notify` is signalled when the GPU has drawn it.
    pub unsafe fn end_scene(&mut self, ctx: *mut g::SceGxmContext, notify: Option<&g::SceGxmNotification>) {
        self.scene.end(ctx, notify);
    }

    /// Copies the scene's own target onto the open display scene.
    pub unsafe fn copy_scene(&self, ctx: *mut g::SceGxmContext) {
        viewport(ctx, W, H);
        self.copy.draw(ctx, self.tri, &taps(), [&[], &[]], [Some(&self.scene.texture), None]);
    }

    /// Runs the quarter-size chain over the frame the display shows: bright, blur across, blur down. `shown`
    /// is that frame's memory: `W` × `H` texels in rows of `stride`. With `look.bloom` off it does nothing.
    pub unsafe fn bloom(&mut self, ctx: *mut g::SceGxmContext, look: &Look, shown: *const core::ffi::c_void, stride: u32) -> Result<(), String> {
        self.lit = false;
        if !look.bloom || look.bloom_gain <= 0.0 || shown.is_null() {
            return Ok(());
        }
        let mut source: g::SceGxmTexture = core::mem::zeroed();
        if g::sceGxmTextureInitLinear(&mut source, shown, g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U8U8U8U8_ABGR, stride, H, 1) < 0 {
            return Ok(());
        }
        g::sceGxmTextureSetMinFilter(&mut source, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        g::sceGxmTextureSetMagFilter(&mut source, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        // Bright: four taps one source texel off the centre, each a bilinear 2 x 2. The frame is the left part of its rows.
        let wide = W as f32 / stride as f32;
        let mut t = taps();
        let (tx, ty) = (1.0 / stride as f32, 1.0 / H as f32);
        for (k, (sx, sy)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].into_iter().enumerate() {
            t[k * 4] = wide;
            t[k * 4 + 2] = sx * tx;
            t[k * 4 + 3] = sy * ty;
        }
        self.a.begin(ctx, 1.0)?;
        viewport(ctx, QW, QH);
        self.bright.draw(ctx, self.tri, &t, [&[look.threshold, 1.0 / (1.0 - look.threshold).max(0.05), 0.0, 0.0], &[]], [Some(&source), None]);
        self.a.end(ctx, None);
        // Blur: across into b, down back into a.
        for (from_a, (dx, dy)) in [(true, (1.0 / QW as f32, 0.0)), (false, (0.0, 1.0 / QH as f32))] {
            let mut t = taps();
            for (k, o) in [0.0f32, 1.3846, -1.3846, 3.2308, -3.2308].into_iter().enumerate() {
                t[k * 4 + 2] = o * dx;
                t[k * 4 + 3] = o * dy;
            }
            let (src, dst) = if from_a { (&self.a.texture as *const g::SceGxmTexture, &mut self.b) } else { (&self.b.texture as *const g::SceGxmTexture, &mut self.a) };
            dst.begin(ctx, 1.0)?;
            viewport(ctx, QW, QH);
            self.blur.draw(ctx, self.tri, &t, [&[], &[]], [Some(&*src), None]);
            dst.end(ctx, None);
        }
        self.lit = true;
        Ok(())
    }

    /// Adds the glow over the open display scene.
    pub unsafe fn add_glow(&self, ctx: *mut g::SceGxmContext, look: &Look) {
        if self.lit {
            viewport(ctx, W, H);
            self.glow.draw(ctx, self.tri, &taps(), [&[look.bloom_gain, 0.0, 0.0, 0.0], &[]], [Some(&self.a.texture), None]);
        }
    }
}
