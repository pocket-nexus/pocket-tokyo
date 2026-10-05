//! What moves in Pocket Tokyo, the same on every device: the camera, the
//! clock and the sun, the traffic. A device supplies the pad and draws what
//! this crate says.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod camera;
#[cfg(feature = "single-float")]
pub mod fastmath;
pub mod math;
pub mod shadow;
pub mod sky;
pub mod tour;
pub mod traffic;

pub use camera::{Camera, Input};
