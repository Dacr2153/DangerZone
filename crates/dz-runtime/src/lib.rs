//! Runtime isolation providers that convert documents in an isolated
//! environment.
//!
//! Corresponds to the `dangerzone` package's `isolation_provider` and `podman`
//! modules. The providers spawn a conversion process (in a container, a
//! disposable qube, or a dummy local process), feed it the input document,
//! and turn the received pixel buffers into a safe PDF.

#![warn(missing_docs)]

pub mod base;
pub mod container;
pub mod container_utils;
pub mod dummy;
pub mod podman;
pub mod qubes;
pub mod updater;
