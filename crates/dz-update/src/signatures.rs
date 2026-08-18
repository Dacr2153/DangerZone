//! Signature verification and storage for the Dangerzone container image.
//!
//! Corresponds to `dangerzone/updater/signatures.py`. The low-level primitives
//! (parsing, `cosign` invocation, local storage) live in
//! `dz-runtime::updater::signatures`, so this module focuses on the
//! orchestration: pulling and verifying remote signatures, loading and
//! preparing air-gapped archives, and deciding whether an upgrade is needed.

use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use dz_core::errors::SignatureError;
use dz_core::util::get_resource_path;
use dz_runtime::container_utils;
use dz_runtime::updater::cosign;
use dz_runtime::updater::signatures as updater_signatures;
use serde_json::Value;

use crate::errors::UpdateError;
use crate::registry;

/// The bundled public key used to verify the image signatures.
pub const DEFAULT_PUBKEY_LOCATION: &str = "/usr/share/dangerzone/dangerzone.pub";

/// The name of the Dangerzone manifest inside air-gapped archives.
pub use updater_signatures::DANGERZONE_MANIFEST;

/// Returns the resolved location of the public key used to verify signatures.
pub fn default_pubkey_location() -> PathBuf {
    updater_signatures::default_pubkey_location()
}

/// Whether the container image was shipped with the build.
pub fn is_container_tar_bundled() -> bool {
    updater_signatures::is_container_tar_bundled()
}

/// Whether a Dangerzone container image is installed locally.
///
/// Both the `last_log_index` state file and an actual image in the container
/// runtime storage must be present; the state file alone is not enough, since
/// the storage can be wiped independently (e.g. `podman system reset`).
pub fn is_container_image_installed() -> bool {
    if !updater_signatures::last_log_index_path().exists() {
        return false;
    }
    container_utils::list_image_digests()
        .map(|digests| !digests.is_empty())
        .unwrap_or(false)
}

/// Returns the signatures of the remote image, as JSON objects.
///
/// Corresponds to `signatures.get_remote_signatures()`: the signatures are
/// downloaded with `cosign download signature` and parsed as JSON.
pub fn get_remote_signatures(image: &str, digest: &str) -> Result<Vec<Value>, UpdateError> {
    let signatures_raw =
        cosign::download_signature(image, digest).map_err(UpdateError::Signature)?;
    let signatures = signatures_raw
        .iter()
        .filter_map(|signature| serde_json::from_str::<Value>(signature).ok())
        .filter(|signature| !signature.is_null())
        .collect::<Vec<_>>();
    if signatures.is_empty() {
        return Err(UpdateError::Signature(SignatureError::NoRemoteSignatures(
            "No signatures found for the image".to_string(),
        )));
    }
    Ok(signatures)
}

/// Verifies that the signatures are valid for the given manifest digest.
pub fn verify_signatures(signatures: &[Value], image_digest: &str) -> Result<(), UpdateError> {
    updater_signatures::verify_signatures(signatures, image_digest, &default_pubkey_location())
        .map_err(UpdateError::Signature)
}

/// Stores the signatures locally so they can be used for future upgrades.
pub fn store_signatures(
    signatures: &[Value],
    manifest_digest: &str,
    update_logindex: bool,
) -> Result<(), UpdateError> {
    updater_signatures::store_signatures(
        signatures,
        manifest_digest,
        &default_pubkey_location(),
        update_logindex,
    )
    .map_err(UpdateError::Signature)
}

/// Verifies the signatures and checks that the incoming log index only moves
/// upwards, mirroring `check_signatures_and_logindex()`.
///
/// Returns the `(last_log_index, incoming_log_index)` pair.
fn check_signatures_and_logindex(
    remote_digest: &str,
    signatures: &[Value],
) -> Result<(i64, i64), UpdateError> {
    verify_signatures(signatures, remote_digest)?;

    let incoming_log_index = updater_signatures::get_log_index_from_signatures(signatures);
    let last_log_index = updater_signatures::get_last_log_index();
    if incoming_log_index < last_log_index {
        return Err(UpdateError::InvalidLogIndex(format!(
            "The incoming log index ({incoming_log_index}) is lower than the last known \
             log index ({last_log_index})"
        )));
    }
    log::info!("Incoming log index: {incoming_log_index}");
    log::info!("Last known log index: {last_log_index}");
    Ok((last_log_index, incoming_log_index))
}

/// Upgrades the local container image to a given remote manifest digest.
///
/// Corresponds to `signatures.upgrade_container_image()`. When `signatures` is
/// absent they are downloaded again from the registry.
pub fn upgrade_container_image(
    remote_digest: &str,
    image_str: &str,
    signatures: Option<&[Value]>,
) -> Result<(), UpdateError> {
    // Avoid downloading the signatures again if we just did it previously.
    let signatures = match signatures {
        Some(signatures) => signatures.to_vec(),
        None => {
            log::info!("Downloading the signatures of the remote image...");
            get_remote_signatures(image_str, remote_digest)?
        }
    };

    log::info!("Verifying the signatures of the remote image...");
    let (last_log_index, incoming_log_index) =
        check_signatures_and_logindex(remote_digest, &signatures)?;

    // If the local log index is the same as the remote one, and a sandbox image
    // has been installed, there is no need to update.
    if incoming_log_index == last_log_index && !container_utils::list_image_digests()?.is_empty() {
        return Err(UpdateError::ImageAlreadyUpToDate);
    }

    log::info!("Pulling the new image...");
    container_utils::container_pull(image_str, remote_digest)?;

    // Now that they are verified, store the signatures.
    log::info!("Storing the signatures of the remote image...");
    store_signatures(&signatures, remote_digest, true)?;
    Ok(())
}

/// Installs the container image bundled in an air-gapped archive, returning the
/// loaded image name and its digest.
///
/// Corresponds to `signatures.upgrade_container_image_airgapped()`. The archive
/// must contain a `dangerzone.json` manifest whose images-only view matches the
/// `index.json`, so that the self-contained signatures cover the loaded image.
pub fn upgrade_container_image_airgapped(
    container_tar: &Path,
    bypass_logindex: bool,
) -> Result<(String, String), UpdateError> {
    let temp_dir = tempfile::tempdir()?;
    let tmp_path = temp_dir.path();

    log::info!("Loading the dangerzone.json manifest...");
    let files = archive_members(container_tar)?;
    let has_dangerzone_manifest = files
        .iter()
        .any(|name| name == &format!("./{}", DANGERZONE_MANIFEST));
    if !has_dangerzone_manifest {
        return Err(UpdateError::InvalidImageArchive);
    }
    log::info!("Found the dangerzone.json manifest in the archive");

    // Sanity check, ensuring that the dangerzone.json file is the same as the
    // index.json with only the images remaining. This is to avoid situations
    // where signatures are checked but the index.json differs, in which case
    // the validity of the signatures wouldn't mean anything.
    archive_unpack(container_tar, tmp_path)?;

    let dz_manifest: Value =
        serde_json::from_slice(&fs::read(tmp_path.join(DANGERZONE_MANIFEST))?)?;
    let index_manifest: Value = serde_json::from_slice(&fs::read(tmp_path.join("index.json"))?)?;

    let expected_manifest = get_images_only_manifest(&dz_manifest);
    if expected_manifest != index_manifest {
        return Err(UpdateError::InvalidDangerzoneManifest);
    }
    log::info!("The dangerzone.json manifest matches the index.json manifest");

    let signature_filename = get_signature_filename(&dz_manifest)?;
    let signature_manifest: Value =
        serde_json::from_slice(&fs::read(tmp_path.join(signature_filename))?)?;

    log::info!("Converting the signatures to a cosign-compatible format");
    let (image_name, signatures) = convert_oci_images_signatures(&signature_manifest, tmp_path)?;
    log::info!("Found image name: {image_name}");

    if !bypass_logindex {
        // Only upgrade if the log index is higher than the last known one.
        let incoming_log_index = updater_signatures::get_log_index_from_signatures(&signatures);
        let last_log_index = updater_signatures::get_last_log_index();
        if incoming_log_index < last_log_index {
            return Err(UpdateError::InvalidLogIndex(
                "The log index is not higher than the last known one".to_string(),
            ));
        }
    }

    if expected_manifest["manifests"]
        .as_array()
        .map(|manifests| manifests.len() > 1)
        .unwrap_or(false)
    {
        return Err(UpdateError::InvalidDangerzoneManifest);
    }
    let image_digest = expected_manifest["manifests"][0]["digest"]
        .as_str()
        .unwrap_or_default()
        .replace("sha256:", "");

    container_utils::load_image_tarball(Some(container_tar))?;
    // Apply the tag manually here, since images downloaded with `cosign save`
    // do not come with the tags attached.
    container_utils::tag_image_by_digest(&image_digest, &image_name)?;

    if let Err(error) = verify_signatures(&signatures, &image_digest)
        .and_then(|()| store_signatures(&signatures, &image_digest, true))
    {
        if matches!(error, UpdateError::Signature(_)) {
            log::info!("Unable to verify the signatures, unload the image");
            let _ = container_utils::delete_image_digests(
                &[format!("sha256:{image_digest}")],
                Some(&image_name),
            );
        }
        return Err(error);
    }

    Ok((image_name, image_digest))
}

/// Extracts the blob `digest` from the archive, mirroring
/// `get_blob_from_archive()`. The archive was previously unpacked into
/// `tmp_path`, so the blob is simply read from disk.
fn get_blob_from_archive(digest: &str, tmp_path: &Path) -> PathBuf {
    tmp_path.join(get_blob(digest))
}

/// Converts OCI image signatures to cosign-compatible signatures.
///
/// Corresponds to `signatures.convert_oci_images_signatures()`, returning the
/// `(image_name, signatures)` pair.
fn convert_oci_images_signatures(
    signatures_manifest: &Value,
    tmp_path: &Path,
) -> Result<(String, Vec<Value>), UpdateError> {
    let layers = signatures_manifest["layers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut signatures = Vec::with_capacity(layers.len());
    for layer in &layers {
        signatures.push(to_cosign_signature(layer, tmp_path)?);
    }

    if signatures.is_empty() {
        return Err(UpdateError::Signature(
            SignatureError::SignatureExtractionError,
        ));
    }

    let payload_location =
        get_blob_from_archive(layers[0]["digest"].as_str().unwrap_or_default(), tmp_path);
    let payload: Value = serde_json::from_slice(&fs::read(payload_location)?)?;
    let image_name = payload["critical"]["identity"]["docker-reference"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    Ok((image_name, signatures))
}

/// Converts a single OCI signature layer to the `cosign download` format.
fn to_cosign_signature(layer: &Value, tmp_path: &Path) -> Result<Value, UpdateError> {
    let bundle: Value = serde_json::from_str(
        layer["annotations"]["dev.sigstore.cosign/bundle"]
            .as_str()
            .unwrap_or_default(),
    )
    .map_err(std::io::Error::other)?;
    let payload_body: Value = serde_json::from_slice(
        &BASE64
            .decode(bundle["Payload"]["body"].as_str().unwrap_or_default())
            .map_err(std::io::Error::other)?,
    )
    .map_err(std::io::Error::other)?;

    let payload_path =
        get_blob_from_archive(layer["digest"].as_str().unwrap_or_default(), tmp_path);
    let payload_b64 = BASE64.encode(fs::read(payload_path)?);

    Ok(serde_json::json!({
        "Base64Signature": payload_body["spec"]["signature"]["content"],
        "Payload": payload_b64,
        "Cert": serde_json::Value::Null,
        "Chain": serde_json::Value::Null,
        "Bundle": bundle,
        "RFC3161Timestamp": serde_json::Value::Null,
    }))
}

/// Filters a manifest down to its image entries, mirroring
/// `_get_images_only_manifest()`.
fn get_images_only_manifest(input: &Value) -> Value {
    let mut output = input.clone();
    let filtered = input["manifests"]
        .as_array()
        .map(|manifests| {
            manifests
                .iter()
                .filter(|manifest| {
                    manifest["annotations"]["kind"]
                        .as_str()
                        .is_some_and(|kind| {
                            kind == "dev.cosignproject.cosign/imageIndex"
                                || kind == "dev.cosignproject.cosign/image"
                        })
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    output["manifests"] = Value::Array(filtered);
    output
}

/// Returns the relative blob path of a digest, mirroring `_get_blob()`.
fn get_blob(digest: &str) -> PathBuf {
    PathBuf::from("blobs")
        .join("sha256")
        .join(digest.replace("sha256:", ""))
}

/// Finds the signature blob inside the dangerzone manifest, mirroring
/// `_get_signature_filename()`.
fn get_signature_filename(input: &Value) -> Result<PathBuf, UpdateError> {
    if let Some(manifests) = input["manifests"].as_array() {
        for manifest in manifests {
            if manifest["annotations"]["kind"].as_str() == Some("dev.cosignproject.cosign/sigs") {
                if let Some(digest) = manifest["digest"].as_str() {
                    return Ok(get_blob(digest));
                }
            }
        }
    }
    Err(UpdateError::Signature(
        SignatureError::SignatureExtractionError,
    ))
}

/// Lists the member paths of a tar archive.
fn archive_members(archive_filename: &Path) -> Result<Vec<String>, UpdateError> {
    let file = fs::File::open(archive_filename)?;
    let mut archive = tar::Archive::new(file);
    Ok(archive
        .entries()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .path()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .collect())
}

/// Unpacks a tar archive into `dest`.
fn archive_unpack(archive_filename: &Path, dest: &Path) -> Result<(), UpdateError> {
    let file = fs::File::open(archive_filename)?;
    let mut archive = tar::Archive::new(file);
    archive.unpack(dest)?;
    Ok(())
}

/// Prepares an archive that can be installed on an air-gapped system.
///
/// Corresponds to `signatures.prepare_airgapped_archive()`. The image is
/// downloaded with `cosign save`, its signatures and attestations are stripped
/// from the `index.json` (podman/docker cannot load them), and the original
/// manifest is stored as `dangerzone.json` so the signatures can be verified
/// when the archive is loaded.
pub fn prepare_airgapped_archive(
    image_name: &str,
    destination: &str,
    architecture: &str,
) -> Result<(), UpdateError> {
    let arch_digest = registry::get_digest_for_arch(image_name, architecture)?;
    let arch_image = registry::replace_image_digest(image_name, &arch_digest, false)?;
    log::info!("Found an image for architecture '{architecture}' at '{arch_image}'");

    let temp_dir = tempfile::tempdir()?;
    let tmp_path = temp_dir.path();

    log::info!("Downloading image {arch_image}. \nIt might take a while.");
    log::debug!("Downloading to temporary directory {}", tmp_path.display());
    cosign::save(&arch_image, tmp_path).map_err(|_| UpdateError::AirgappedImageDownload)?;
    cosign::verify_local_image(tmp_path, &default_pubkey_location())?;

    // Read from index.json, save it as dangerzone.json, and then change the
    // index.json contents to only contain images.
    let original_index_json: Value =
        serde_json::from_slice(&fs::read(tmp_path.join("index.json"))?)?;
    fs::write(
        tmp_path.join(DANGERZONE_MANIFEST),
        serde_json::to_vec(&original_index_json)?,
    )?;

    let new_index_json = get_images_only_manifest(&original_index_json);
    fs::write(
        tmp_path.join("index.json"),
        serde_json::to_vec(&new_index_json)?,
    )?;

    let file = fs::File::create(destination)?;
    let mut builder = tar::Builder::new(file);
    builder.append_dir_all(".", tmp_path)?;
    builder.finish()?;
    Ok(())
}

/// Checks the remote registry for updates, downloading and verifying the
/// signatures.
///
/// Corresponds to `signatures.get_remote_digest_and_logindex()`, returning the
/// `(remote_digest, remote_log_index, signatures)` triple.
pub fn get_remote_digest_and_logindex(
    image_str: &str,
) -> Result<(String, i64, Vec<Value>), UpdateError> {
    log::info!("Get manifest digests");
    let remote_digest = registry::get_manifest_digest(image_str)?;

    log::info!("Get remote signatures");
    let signatures = get_remote_signatures(image_str, &remote_digest)?;

    log::info!("Verify signatures");
    verify_signatures(&signatures, &remote_digest)?;

    log::info!("Getting log index from signatures");
    let remote_log_index = updater_signatures::get_log_index_from_signatures(&signatures);
    Ok((remote_digest, remote_log_index, signatures))
}

/// Installs a container tarball stored locally, and returns its digest.
///
/// Corresponds to `signatures.install_local_container_tar()`. In dev mode the
/// signature checks can be bypassed with the `DANGERZONE_BYPASS_SIG_CHECKS`
/// environment variable.
pub fn install_local_container_tar() -> Result<String, UpdateError> {
    let tarball_path = get_resource_path("container.tar").ok_or_else(|| {
        UpdateError::Io(std::io::Error::other("container.tar resource not found"))
    })?;
    log::debug!("Installing container image {}", tarball_path.display());
    if bypass_signature_checks() {
        return Ok(container_utils::load_image_tarball(Some(&tarball_path))?);
    }
    let (_, image_digest) = upgrade_container_image_airgapped(&tarball_path, false)?;
    Ok(image_digest)
}

/// Whether signature checks are bypassed in dev mode.
///
/// This is the signatures-module variant of the escape hatch; it only applies
/// when a dev build is running and the `DANGERZONE_BYPASS_SIG_CHECKS`
/// environment variable is set, mirroring `signatures.bypass_signature_checks()`.
pub fn bypass_signature_checks() -> bool {
    if dz_core::util::is_dev() {
        if let Ok(value) = std::env::var("DANGERZONE_BYPASS_SIG_CHECKS") {
            if value == "1" || value == "true" {
                log::warn!(
                    "Bypassing signature checks because dev mode is detected and the \
                     DANGERZONE_BYPASS_SIG_CHECKS environment variable is set"
                );
                return true;
            }
        }
    }
    false
}

/// Verifies that the local image signature(s) match the embedded public key.
///
/// Corresponds to `signatures.verify_local_image()`. Returns `Ok(true)` when
/// the image is present and verified, `Ok(false)` when the image does not exist
/// locally, and an error when the signatures cannot be verified.
pub fn verify_local_image(image: &str) -> Result<bool, UpdateError> {
    log::info!(
        "Verifying local image {image} against pubkey {}",
        default_pubkey_location().display()
    );
    let image_digest = container_utils::get_local_image_digest(Some(image)).map_err(|_| {
        UpdateError::ImageNotFound(format!("The image {image} does not exist locally"))
    })?;
    log::debug!("Image digest: {image_digest}");
    updater_signatures::load_and_verify_signatures(
        &image_digest,
        &default_pubkey_location(),
        false,
    )
    .map_err(|error| {
        UpdateError::Signature(SignatureError::SignatureVerificationError(format!(
            "Failed to verify the local image {image}: {error}"
        )))
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_location_is_defined() {
        assert!(!DEFAULT_PUBKEY_LOCATION.is_empty());
    }

    #[test]
    fn images_only_manifest_keeps_only_images() {
        let manifest = serde_json::json!({
            "manifests": [
                {
                    "digest": "sha256:aaa",
                    "annotations": { "kind": "dev.cosignproject.cosign/image" }
                },
                {
                    "digest": "sha256:bbb",
                    "annotations": { "kind": "dev.cosignproject.cosign/sigs" }
                },
                {
                    "digest": "sha256:ccc",
                    "annotations": { "kind": "dev.cosignproject.cosign/attestation" }
                }
            ]
        });
        let filtered = get_images_only_manifest(&manifest);
        let digests = filtered["manifests"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["digest"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(digests, vec!["sha256:aaa"]);
    }

    #[test]
    fn signature_filename_is_the_sigs_blob() {
        let manifest = serde_json::json!({
            "manifests": [
                { "digest": "sha256:aaa", "annotations": { "kind": "dev.cosignproject.cosign/image" } },
                { "digest": "sha256:bbb", "annotations": { "kind": "dev.cosignproject.cosign/sigs" } }
            ]
        });
        let filename = get_signature_filename(&manifest).unwrap();
        assert_eq!(filename, PathBuf::from("blobs/sha256/bbb"));
    }

    #[test]
    fn signature_filename_is_an_error_without_sigs() {
        let manifest = serde_json::json!({
            "manifests": [
                { "digest": "sha256:aaa", "annotations": { "kind": "dev.cosignproject.cosign/image" } }
            ]
        });
        assert!(matches!(
            get_signature_filename(&manifest),
            Err(UpdateError::Signature(
                SignatureError::SignatureExtractionError
            ))
        ));
    }

    #[test]
    fn blob_paths_strip_the_sha256_prefix() {
        assert_eq!(
            get_blob("sha256:deadbeef"),
            PathBuf::from("blobs/sha256/deadbeef")
        );
    }

    #[test]
    fn signature_checks_are_not_bypassed_by_default() {
        std::env::remove_var("DANGERZONE_BYPASS_SIG_CHECKS");
        assert!(!bypass_signature_checks());
    }

    #[test]
    fn container_tar_is_not_bundled() {
        assert!(!is_container_tar_bundled());
    }
}
