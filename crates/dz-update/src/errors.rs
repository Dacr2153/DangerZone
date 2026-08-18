//! Errors raised by the update mechanism.
//!
//! Corresponds to `dangerzone/updater/errors.py`. The Python exception classes
//! are flattened into nested `thiserror` enums, mirroring the approach used in
//! `dz-core::errors`. The base classes `SignatureError` and `RegistryError`
//! become enums of their subclasses, and are re-exposed through
//! [`UpdateError::Signature`] / [`UpdateError::Registry`]. `SignatureError` is
//! shared with `dz-core` so that the runtime can verify local images without a
//! dependency cycle; it is re-exported here for convenience.

pub use dz_core::errors::SignatureError;

use dz_core::errors::ContainerError;

/// Errors raised while upgrading or verifying the Dangerzone sandbox image.
///
/// Corresponds to `UpdaterError` and its direct subclasses.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// An upgrade was required but everything is already up to date.
    #[error("An upgrade was required but everything is already up to date")]
    ImageAlreadyUpToDate,
    /// A verification of the local container image was requested, but no image
    /// could be found.
    #[error("{0}")]
    ImageNotFound(String),
    /// The incoming log index is not greater than the previous one.
    #[error("{0}")]
    InvalidLogIndex(String),
    /// An invalid archive format was passed.
    #[error(
        "An invalid archive format was passed. Archives should contain a \
         `dangerzone.json` file. The proper way to gather these archives is to \
         use: `dangerzone-image prepare-archive` in your terminal."
    )]
    InvalidImageArchive,
    /// The `dangerzone.json` manifest does not match the `index.json` manifest
    /// in a container image, so the image may have been tampered with.
    #[error(
        "The dangerzone.json manifest does not match the index.json manifest in \
         the container image. This could mean that the container image has been \
         tampered with and is not safe to load."
    )]
    InvalidDangerzoneManifest,
    /// Unable to download the container image using `cosign download`.
    #[error("Unable to download the container image using cosign download")]
    AirgappedImageDownload,
    /// A signature-related error occurred.
    #[error(transparent)]
    Signature(#[from] SignatureError),
    /// A container registry error occurred.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// A generic HTTP error occurred.
    #[error("{0}")]
    Http(String),
    /// An error raised by the container runtime.
    #[error(transparent)]
    Container(#[from] ContainerError),
    /// A JSON payload could not be parsed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// An underlying I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Errors found while interacting with the container registry.
///
/// Corresponds to `RegistryError` and its subclasses.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The queried image is not a multi-arch image.
    #[error("The queried image is not a multi-arch image")]
    InvalidMultiArchImage,
    /// The required architecture was not found in the provided manifest.
    #[error("The required architecture was not found in the provided manifest")]
    ArchitectureNotFound,
    /// An HTTP request to the registry failed.
    #[error("{0}")]
    Http(String),
    /// The registry manifest could not be parsed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_match_python_originals() {
        assert_eq!(
            UpdateError::ImageAlreadyUpToDate.to_string(),
            "An upgrade was required but everything is already up to date"
        );
        assert_eq!(
            UpdateError::InvalidImageArchive.to_string(),
            "An invalid archive format was passed. Archives should contain a \
             `dangerzone.json` file. The proper way to gather these archives is \
             to use: `dangerzone-image prepare-archive` in your terminal."
        );
        assert_eq!(
            UpdateError::AirgappedImageDownload.to_string(),
            "Unable to download the container image using cosign download"
        );
    }

    #[test]
    fn signature_errors_carry_a_message() {
        assert_eq!(
            UpdateError::Signature(SignatureError::SignatureVerificationError(
                "boom".to_string()
            ))
            .to_string(),
            "boom"
        );
        assert_eq!(
            SignatureError::SignatureExtractionError.to_string(),
            "The signatures do not match the expected format"
        );
    }

    #[test]
    fn registry_errors_carry_a_message() {
        assert_eq!(
            UpdateError::Registry(RegistryError::InvalidMultiArchImage).to_string(),
            "The queried image is not a multi-arch image"
        );
        assert_eq!(
            RegistryError::ArchitectureNotFound.to_string(),
            "The required architecture was not found in the provided manifest"
        );
    }
}
