//! Interface with the embedded `cosign` binary.
//!
//! Corresponds to `dangerzone/updater/cosign.py`. The binary is bundled with
//! the build under `share/vendor/cosign`; when it is missing, every operation
//! fails with [`SignatureError::CosignNotInstalledError`].
//!
//! This module lives in `dz-runtime` because the container provider verifies a
//! local image before starting a conversion, and keeping the signing tool here
//! lets the updater crate reuse it without creating a dependency cycle.

use std::path::{Path, PathBuf};
use std::process::Command;

use dz_core::errors::SignatureError;
use dz_core::util::{get_resource_path, get_tails_socks_proxy, linux_system_is};

/// Returns the path of the bundled `cosign` binary.
pub fn cosign_binary() -> Result<PathBuf, SignatureError> {
    get_resource_path("vendor/cosign").ok_or(SignatureError::CosignNotInstalledError)
}

/// Runs a `cosign` command, mirroring `_cosign_run()`.
///
/// Registry authentication is disabled by pointing the auth-related environment
/// variables to non-existent files, so that any credentials configured on the
/// system are not used. When `pin_rekor_key` is set, the bundled Rekor public
/// key is used for offline verification.
///
/// On failure, the `cosign` stderr is returned as the error message; callers
/// wrap it in the appropriate [`SignatureError`] variant.
fn cosign_run(cmd: &[&str], disable_auth: bool, pin_rekor_key: bool) -> Result<String, String> {
    let binary = cosign_binary().map_err(|e| e.to_string())?;
    let mut command = Command::new(binary);
    command.args(cmd);

    let mut extra_env: Vec<(&str, String)> = Vec::new();
    if disable_auth {
        extra_env.push(("REGISTRY_AUTH_FILE", "does-not-exist".to_string()));
        extra_env.push(("DOCKER_CONFIG", "does-not-exist".to_string()));
    }
    if pin_rekor_key {
        if let Some(rekor_pub_key) = get_resource_path("rekor.pub") {
            extra_env.push((
                "SIGSTORE_REKOR_PUBLIC_KEY",
                rekor_pub_key.to_string_lossy().into_owned(),
            ));
        }
    }
    if linux_system_is(&["Tails"]) {
        extra_env.push(("HTTPS_PROXY", get_tails_socks_proxy()));
    }
    command.envs(extra_env);

    let output = command
        .output()
        .map_err(|e| format!("could not run cosign: {e}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("cosign command failed with status: {}", output.status)
        } else {
            message
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Verifies a blob signature against the given public key.
///
/// Corresponds to `cosign.verify_blob()`. The signature is passed in the
/// `cosign verify-blob --bundle` format, along with the signed payload.
pub fn verify_blob(
    pubkey: &Path,
    bundle_file: &Path,
    payload_file: &Path,
) -> Result<(), SignatureError> {
    cosign_run(
        &[
            "verify-blob",
            "--offline",
            "--key",
            &pubkey.to_string_lossy(),
            "--bundle",
            &bundle_file.to_string_lossy(),
            &payload_file.to_string_lossy(),
        ],
        true,
        true,
    )
    .map(|_| ())
    .map_err(|e| {
        SignatureError::SignatureVerificationError(format!("Failed to verify signature: {e}"))
    })
}

/// Verifies a local OCI image folder against the given public key.
///
/// Corresponds to `cosign.verify_local_image()`.
pub fn verify_local_image(oci_image_folder: &Path, pubkey: &Path) -> Result<(), SignatureError> {
    cosign_run(
        &[
            "verify",
            "--key",
            &pubkey.to_string_lossy(),
            "--offline",
            "--local-image",
            &oci_image_folder.to_string_lossy(),
        ],
        true,
        true,
    )
    .map(|_| ())
    .map_err(|e| {
        SignatureError::SignatureVerificationError(format!(
            "Failed to verify signature of local image: {e}"
        ))
    })
}

/// Downloads the signatures of an image from the registry.
///
/// Corresponds to `cosign.download_signature()`. Each returned string is a
/// JSON-encoded signature.
pub fn download_signature(image: &str, digest: &str) -> Result<Vec<String>, SignatureError> {
    cosign_run(
        &["download", "signature", &format!("{image}@sha256:{digest}")],
        true,
        false,
    )
    .map(|output| {
        output
            .trim()
            .split('\n')
            .map(str::to_string)
            .filter(|line| !line.is_empty())
            .collect()
    })
    .map_err(SignatureError::NoRemoteSignatures)
}

/// Saves an image from the registry into an OCI layout directory.
///
/// Corresponds to `cosign.save()`. The caller maps failures to
/// `AirgappedImageDownloadError`, mirroring the original.
pub fn save(arch_image: &str, destination: &Path) -> Result<(), SignatureError> {
    cosign_run(
        &["save", arch_image, "--dir", &destination.to_string_lossy()],
        true,
        false,
    )
    .map(|_| ())
    .map_err(SignatureError::SignatureVerificationError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_cosign_binary_is_reported() {
        // In a dev build the binary is looked up under share/vendor/cosign,
        // which is not shipped with the source tree, so the lookup must fail.
        if dz_core::util::is_dev() {
            assert!(matches!(
                cosign_binary(),
                Err(SignatureError::CosignNotInstalledError)
            ));
        }
    }
}
