//! Pocket Tokyo drawn with wgpu: the city of the handheld builds in a browser
//! tab over WebGPU, and on the build machine, where a frame can be written to
//! a file.
//!
//! The city, the flight and what a frame draws are the handheld core's
//! (`tokyo_core`, the source of `n3ds/core`); the pack is the iPod touch's
//! (`profiles/ipod60.json`), read over HTTP a range at a time ([`pack`]); the
//! programs are the iPod touch's, in WGSL ([`render`]). [`app`] is the shell
//! around them. What is not this game's is `pocket_web_wgpu` (`kernel/`).

pub mod app;
pub mod pack;
pub mod render;
#[cfg(target_arch = "wasm32")]
mod web;
