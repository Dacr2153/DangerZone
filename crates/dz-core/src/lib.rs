//! Core logic shared by the Dangerzone applications.
//!
//! The original `__init__.py` also patched the stdlib to capture background
//! output, called `multiprocessing.freeze_support()`, and vendored PyMuPDF
//! libraries. Those steps are PyInstaller/Python-runtime specific and have no
//! Rust equivalent; the meaningful remaining behaviour is the development-build
//! detection, exposed here through [`util::is_dev`].

#![warn(missing_docs)]

pub mod document;
pub mod errors;
pub mod logic;
pub mod settings;
pub mod shutdown;
pub mod startup;
pub mod stubs;
pub mod updater;
pub mod util;
