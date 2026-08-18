//! Signed update mechanism for the Dangerzone application and its sandbox image.
//!
//! Corresponds to the `dangerzone.updater` package. The registry, signature and
//! release-check handling live here, along with the concrete installer and
//! update checker ([`updater::Updater`]) that the applications wire into their
//! startup tasks.

#![warn(missing_docs)]

pub mod cli;
pub mod errors;
pub mod manifest;
pub mod registry;
pub mod signatures;
pub mod updater;
