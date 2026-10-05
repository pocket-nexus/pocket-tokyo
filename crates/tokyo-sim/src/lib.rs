//! What moves in Pocket Tokyo, the same on every device: the camera, the
//! clock and the sun, the traffic. A device supplies the pad and draws what
//! this crate says.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "hw-sqrt", allow(internal_features), feature(core_intrinsics))]

extern crate alloc;

pub mod camera;
#[cfg(feature = "single-float")]
pub mod fastmath;
pub mod flight;
pub mod mat;
pub mod math;
pub mod shadow;
pub mod sky;
pub mod text;
pub mod tour;
pub mod traffic;
pub mod view;

pub use camera::{Camera, Input};
