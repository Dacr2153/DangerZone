//! Verification of the locally available container image.
//!
//! Corresponds to `dangerzone/updater.py` and the parts of
//! `dangerzone/updater/signatures.py` that the container provider needs before
//! starting a conversion. The full signature handling is implemented in the
//! [`signatures`] and [`cosign`] submodules; this module wires the pieces that
//! run when a container is about to be started.

pub mod cosign;
pub mod signatures;

pub use signatures::{default_pubkey_location, is_container_tar_bundled};

use dz_core::errors::ContainerError;

/// Whether signature verification is bypassed.
///
/// Mirrors `updater.bypass_signature_checks()`, which reads the
/// `DANGERZONE_BYPASS_SIGNATURE_VERIFICATION` environment variable.
pub fn bypass_signature_checks() -> bool {
    std::env::var("DANGERZONE_BYPASS_SIGNATURE_VERIFICATION")
        .map(|value| value == "1")
        .unwrap_or(false)
}

/// Verifies that a locally downloaded container image is signed.
///
/// Corresponds to `updater.verify_local_image()`. The stored signatures for the
/// given digest are loaded and verified against the bundled public key.
pub fn verify_local_image(image_digest: &str) -> Result<(), ContainerError> {
    signatures::verify_local_image(image_digest, &signatures::default_pubkey_location())
        .map_err(ContainerError::Signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_checks_are_not_bypassed_by_default() {
        std::env::remove_var("DANGERZONE_BYPASS_SIGNATURE_VERIFICATION");
        assert!(!bypass_signature_checks());
    }

    #[test]
    fn container_tar_is_not_bundled() {
        assert!(!is_container_tar_bundled());
    }

    #[test]
    fn pubkey_location_is_resolved_from_resources() {
        // In a dev build the public key lives under share/ in the project root.
        if dz_core::util::is_dev() {
            assert!(default_pubkey_location()
                .to_string_lossy()
                .ends_with("freedomofpress-dangerzone.pub"));
        }
    }
}
