//! Reconstructs a safe PDF from the raw RGB page buffers produced by the
//! conversion sandbox.
//!
//! This crate corresponds to the host-side `pixels_to_pdf` phase of the
//! original `dangerzone.conversion.pixels_to_pdf` module. It consumes the
//! untrusted pixel data produced inside the sandbox and builds a brand-new PDF
//! from scratch, so that no content of the original document survives. The
//! output is then re-parsed by [`validator`] to prove it contains none of the
//! active content that the sanitizer is meant to strip (JavaScript, embedded
//! files, launch actions).
//!
//! The original code uses PyMuPDF (`fitz`) for both the page assembly and the
//! final save. This port writes the PDF directly with a minimal, dependency
//! free writer (see [`pdf`]) and applies Flate compression to keep the
//! output small.

#![warn(missing_docs)]

pub mod compression;
pub mod metadata;
pub mod pdf;
pub mod validator;
