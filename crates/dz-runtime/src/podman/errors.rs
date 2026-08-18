//! Errors raised by the Podman integration.
//!
//! Corresponds to `dangerzone/podman/errors.py`.

/// An error reported by a Podman command that failed.
///
/// Corresponds to `CommandError`, which the command runner raises when a
/// `podman` invocation exits with a non-zero status.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CommandError(pub String);

/// An error related to managing the Dangerzone Podman machine.
///
/// Corresponds to `PodmanError`.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PodmanError(pub String);
