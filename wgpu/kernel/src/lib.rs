//! The browser's device kernel for a Pocket3D game drawn with wgpu.
//!
//! A game owns its scene: its pack, its pipelines, what a frame draws. What
//! every such game needs around that is here:
//!
//! - [`gpu`]: the device, and the [`gpu::Screen`] a frame goes to: a canvas in
//!   a browser tab (WebGPU), or a texture on the build machine that is read
//!   back, so the same renderer writes a picture to a file.
//! - [`picture`]: pictures of 16 bits a texel (`r5 g6 b5`, as the handheld
//!   packs store them) as textures WebGPU can read.
//! - [`source`]: ranges of a file, over HTTP in a tab and from disk elsewhere.
//! - [`task`]: something that waits, started from a frame.
//!
//! The page's side (the title card, the frame loop, the keys) is
//! `web/pocket3d-shell.js`.

pub mod gpu;
pub mod picture;
pub mod source;
pub mod task;

pub use wgpu;
