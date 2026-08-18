//! Conversion protocol shared between the Dangerzone client and its sandbox.
//!
//! The original `conversion/` package contained the client-side conversion
//! logic and the error codes that cross the sandbox boundary. This crate keeps
//! the [`errors`] module always compiled, and exposes the container-side
//! conversion pipeline (`doc_to_pixels`, `format_detect`, `image`, `office`,
//! `pdf`) behind the `sandbox` cargo feature, which only the `dz-convert`
//! binary enables.

#![warn(missing_docs)]

pub mod errors;

#[cfg(feature = "sandbox")]
pub mod doc_to_pixels;
#[cfg(feature = "sandbox")]
pub mod epub;
#[cfg(feature = "sandbox")]
pub mod format_detect;
#[cfg(feature = "sandbox")]
pub mod image;
#[cfg(feature = "sandbox")]
pub mod office;
#[cfg(feature = "sandbox")]
pub mod pdf;
#[cfg(feature = "sandbox")]
pub mod svg;
