//! Signature verification and storage for the container image.
//!
//! Corresponds to `dangerzone/updater/signatures.py`. Only the pure parts of
//! the module are implemented here (parsing, verification, storage and log
//! index bookkeeping); the registry-facing operations (pulling, air-gapped
//! archives) build on these primitives in the updater crate.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use dz_core::errors::SignatureError;
use dz_core::util::get_resource_path;
use sha2::{Digest, Sha256};

/// The last log index that is accepted when no state file exists yet.
///
/// Mirrors `dangerzone/updater/log_index.py`.
pub const LAST_KNOWN_LOG_INDEX: i64 = 2023689270;

/// The name of the Dangerzone manifest inside air-gapped archives.
pub const DANGERZONE_MANIFEST: &str = "dangerzone.json";

/// The directory where verified signatures are stored.
///
/// Corresponds to `SIGNATURES_PATH`, i.e. `~/.local/share/dangerzone/signatures`.
pub fn signatures_path() -> PathBuf {
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dangerzone")
        .join("signatures")
}

/// The file that tracks the last processed log index.
pub fn last_log_index_path() -> PathBuf {
    signatures_path().join("last_log_index")
}

/// Returns the location of the public key used to verify the image signatures.
pub fn default_pubkey_location() -> PathBuf {
    get_resource_path("freedomofpress-dangerzone.pub")
        .unwrap_or_else(|| PathBuf::from("freedomofpress-dangerzone.pub"))
}

/// Returns whether the container image was shipped with the build.
///
/// The Qubes-specific "dangerzone-full" builds ship the container image as a
/// bundle; the default package does not.
pub fn is_container_tar_bundled() -> bool {
    get_resource_path("container.tar")
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// A cosign signature, mirroring the `Signature` dataclass of `signatures.py`.
#[derive(Debug, Clone)]
pub struct Signature {
    signature: serde_json::Value,
}

impl Signature {
    /// Wraps a raw cosign signature (a JSON object).
    pub fn new(signature: serde_json::Value) -> Self {
        Self { signature }
    }

    /// The raw JSON object of the signature.
    pub fn as_value(&self) -> &serde_json::Value {
        &self.signature
    }

    /// The decoded payload bytes of the signature.
    pub fn payload_bytes(&self) -> Result<Vec<u8>, SignatureError> {
        let payload = self
            .signature
            .get("Payload")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                SignatureError::SignatureVerificationError(
                    "Signature payload is missing or not a string".to_string(),
                )
            })?;
        BASE64.decode(payload).map_err(|e| {
            SignatureError::SignatureVerificationError(format!(
                "Could not decode the signature payload: {e}"
            ))
        })
    }

    /// The decoded payload as JSON.
    pub fn payload(&self) -> Result<serde_json::Value, SignatureError> {
        let bytes = self.payload_bytes()?;
        serde_json::from_slice(&bytes).map_err(|e| {
            SignatureError::SignatureVerificationError(format!(
                "Could not parse the signature payload: {e}"
            ))
        })
    }

    /// The digest of the image this signature was made over, without the
    /// `sha256:` prefix.
    pub fn manifest_digest(&self) -> Result<String, SignatureError> {
        let payload = self.payload()?;
        let full_digest = payload["critical"]["image"]["docker-manifest-digest"]
            .as_str()
            .ok_or_else(|| {
                SignatureError::SignatureVerificationError(
                    "Missing docker-manifest-digest in the signature payload".to_string(),
                )
            })?;
        Ok(full_digest.replace("sha256:", ""))
    }

    /// The log index of the signature.
    pub fn log_index(&self) -> Result<i64, SignatureError> {
        self.signature["Bundle"]["Payload"]["logIndex"]
            .as_i64()
            .ok_or_else(|| {
                SignatureError::SignatureVerificationError(
                    "Missing log index in the signature bundle".to_string(),
                )
            })
    }

    /// Converts a `cosign download` signature to the format expected by
    /// `cosign verify-blob --bundle`, mirroring `Signature.to_bundle()`.
    pub fn to_bundle(&self) -> Result<serde_json::Value, SignatureError> {
        let bundle = self.signature.get("Bundle").cloned().ok_or_else(|| {
            SignatureError::SignatureVerificationError(
                "Missing bundle in the signature".to_string(),
            )
        })?;
        let payload = bundle.get("Payload").cloned().ok_or_else(|| {
            SignatureError::SignatureVerificationError(
                "Missing payload in the signature bundle".to_string(),
            )
        })?;
        let sig = &self.signature;
        let value = serde_json::json!({
            "base64Signature": sig.get("Base64Signature"),
            "Payload": sig.get("Payload"),
            "cert": sig.get("Cert"),
            "chain": sig.get("Chain"),
            "rekorBundle": {
                "SignedEntryTimestamp": bundle.get("SignedEntryTimestamp"),
                "Payload": {
                    "body": payload.get("body"),
                    "integratedTime": payload.get("integratedTime"),
                    "logIndex": payload.get("logIndex"),
                    "logID": payload.get("logID"),
                },
            },
            "RFC3161Timestamp": sig.get("RFC3161Timestamp"),
        });
        Ok(value)
    }
}

/// Verifies that a single signature matches the public key and image digest.
///
/// Corresponds to `signatures.verify_signature()`. The payload digest must
/// match `image_digest`, and the signature must verify against `pubkey` using
/// the `cosign verify-blob` command.
pub fn verify_signature(
    signature: &serde_json::Value,
    image_digest: &str,
    pubkey: &Path,
) -> Result<(), SignatureError> {
    let sig_obj = Signature::new(signature.clone());
    let payload_digest = sig_obj.manifest_digest().map_err(|e| {
        SignatureError::SignatureVerificationError(format!(
            "Unable to extract the payload digest from the signature: {e}"
        ))
    })?;
    if payload_digest != image_digest {
        return Err(SignatureError::SignatureMismatch(format!(
            "The given signature does not match the expected image digest ({payload_digest}, {image_digest})"
        )));
    }

    // Write the bundle and payload to temporary files, then let cosign verify
    // them. The files are read by the `cosign` subprocess, which runs to
    // completion before the temporary files are dropped.
    let signature_file = tempfile::NamedTempFile::new().map_err(wrap)?;
    let payload_file = tempfile::NamedTempFile::new().map_err(wrap)?;
    fs::write(
        signature_file.path(),
        serde_json::to_vec(&sig_obj.to_bundle()?).map_err(wrap)?,
    )
    .map_err(wrap)?;
    fs::write(payload_file.path(), sig_obj.payload_bytes()?).map_err(wrap)?;

    super::cosign::verify_blob(pubkey, signature_file.path(), payload_file.path())
}

/// Verifies a list of signatures against the public key and image digest.
///
/// Corresponds to `signatures.verify_signatures()`. At least one signature must
/// be present.
pub fn verify_signatures(
    signatures: &[serde_json::Value],
    image_digest: &str,
    pubkey: &Path,
) -> Result<(), SignatureError> {
    if signatures.is_empty() {
        return Err(SignatureError::SignatureVerificationError(
            "No signatures found".to_string(),
        ));
    }
    for signature in signatures {
        verify_signature(signature, image_digest, pubkey)?;
    }
    Ok(())
}

/// Returns the maximum log index found in a list of signatures.
///
/// Signatures without a parseable log index are skipped, mirroring the reducer
/// of `get_log_index_from_signatures()`.
pub fn get_log_index_from_signatures(signatures: &[serde_json::Value]) -> i64 {
    signatures
        .iter()
        .filter_map(|signature| signature["Bundle"]["Payload"]["logIndex"].as_i64())
        .max()
        .unwrap_or(0)
}

/// Returns the last known log index, or [`LAST_KNOWN_LOG_INDEX`] when no state
/// file exists.
pub fn get_last_log_index() -> i64 {
    let path = last_log_index_path();
    let _ = fs::create_dir_all(signatures_path());
    if !path.exists() {
        return LAST_KNOWN_LOG_INDEX;
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|content| content.trim().parse::<i64>().ok())
        .unwrap_or(LAST_KNOWN_LOG_INDEX)
}

/// Persists the given log index as the last processed one.
pub fn write_log_index(log_index: i64) -> Result<(), std::io::Error> {
    fs::create_dir_all(signatures_path())?;
    fs::write(last_log_index_path(), log_index.to_string())?;
    Ok(())
}

/// Computes the sha256 digest of a file, as a hex string.
pub fn get_file_digest(path: &Path) -> Result<String, SignatureError> {
    let content = fs::read(path).map_err(wrap)?;
    Ok(to_hex(&Sha256::digest(&content)))
}

/// Loads the signatures of an image from the local filesystem and verifies
/// them, mirroring `load_and_verify_signatures()`.
pub fn load_and_verify_signatures(
    image_digest: &str,
    pubkey: &Path,
    bypass_verification: bool,
) -> Result<Vec<serde_json::Value>, SignatureError> {
    let pubkey_signatures = signatures_path().join(get_file_digest(pubkey)?);
    if !pubkey_signatures.exists() {
        return Err(SignatureError::SignaturesFolderDoesNotExist(format!(
            "Cannot find a '{}' folder. You might need to download the image signatures first.",
            pubkey_signatures.display()
        )));
    }

    let signatures_file = pubkey_signatures.join(format!("{image_digest}.json"));
    if !signatures_file.exists() {
        return Err(SignatureError::LocalSignatureNotFound(format!(
            "Cannot find a '{}' file. You might need to download the image signatures first.",
            signatures_file.display()
        )));
    }

    let content = fs::read_to_string(&signatures_file).map_err(wrap)?;
    log::debug!("Loading signatures from {}", signatures_file.display());
    let signatures: Vec<serde_json::Value> = serde_json::from_str(&content).map_err(wrap)?;

    if !bypass_verification {
        verify_signatures(&signatures, image_digest, pubkey)?;
    }
    Ok(signatures)
}

/// Stores signatures locally, mirroring `signatures.store_signatures()`.
///
/// The signatures are stored under
/// `SIGNATURES_PATH/<pubkey-digest>/<image-digest>.json`, and the log index is
/// advanced when `update_logindex` is set.
pub fn store_signatures(
    signatures: &[serde_json::Value],
    image_digest: &str,
    pubkey: &Path,
    update_logindex: bool,
) -> Result<(), SignatureError> {
    let digests = signatures
        .iter()
        .map(signature_digest)
        .collect::<Result<Vec<_>, _>>()?;
    let unique_digests: HashSet<&String> = digests.iter().collect();
    if unique_digests.len() != 1 {
        return Err(SignatureError::SignatureMismatch(
            "Signatures do not share the same image digest".to_string(),
        ));
    }
    if format!("sha256:{image_digest}") != digests[0] {
        return Err(SignatureError::SignatureMismatch(format!(
            "Signatures do not match the given image digest (sha256:{image_digest}, {})",
            digests[0]
        )));
    }

    let pubkey_signatures = signatures_path().join(get_file_digest(pubkey)?);
    fs::create_dir_all(&pubkey_signatures).map_err(wrap)?;
    let signatures_file = pubkey_signatures.join(format!("{image_digest}.json"));
    log::info!(
        "Storing signatures for {image_digest} in {}",
        signatures_file.display()
    );
    let json = serde_json::to_vec(signatures).map_err(wrap)?;
    fs::write(&signatures_file, json).map_err(wrap)?;

    if update_logindex {
        write_log_index(get_log_index_from_signatures(signatures)).map_err(wrap)?;
    }
    Ok(())
}

/// Verifies that the locally stored signatures match the given image digest.
///
/// Corresponds to `signatures.verify_local_image()` with an explicit digest.
pub fn verify_local_image(image_digest: &str, pubkey: &Path) -> Result<(), SignatureError> {
    log::debug!("Verifying signatures for image digest {image_digest}");
    load_and_verify_signatures(image_digest, pubkey, false)?;
    Ok(())
}

/// Extracts the `docker-manifest-digest` from a signature's payload.
fn signature_digest(signature: &serde_json::Value) -> Result<String, SignatureError> {
    let payload_b64 = signature
        .get("Payload")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            SignatureError::SignatureVerificationError(
                "Signature payload is missing or not a string".to_string(),
            )
        })?;
    let payload_bytes = BASE64.decode(payload_b64).map_err(wrap)?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).map_err(wrap)?;
    payload["critical"]["image"]["docker-manifest-digest"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| {
            SignatureError::SignatureVerificationError(
                "Missing docker-manifest-digest in the signature payload".to_string(),
            )
        })
}

/// Formats a byte slice as a lowercase hexadecimal string.
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Wraps an arbitrary error as a signature verification error.
fn wrap<E: std::fmt::Display>(error: E) -> SignatureError {
    SignatureError::SignatureVerificationError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_signature() -> serde_json::Value {
        // A minimal cosign-style signature whose payload names a fake digest.
        let payload = serde_json::json!({
            "critical": {
                "image": { "docker-manifest-digest": "sha256:abc123" }
            }
        });
        serde_json::json!({
            "Base64Signature": "AAAA",
            "Payload": BASE64.encode(serde_json::to_vec(&payload).unwrap()),
            "Cert": null,
            "Chain": null,
            "Bundle": {
                "SignedEntryTimestamp": "AAAA",
                "Payload": {
                    "body": "AAAA",
                    "integratedTime": 1,
                    "logIndex": 42,
                    "logID": "AAAA"
                }
            },
            "RFC3161Timestamp": null
        })
    }

    #[test]
    fn signature_payload_and_digest_are_extracted() {
        let sig = Signature::new(sample_signature());
        assert_eq!(sig.manifest_digest().unwrap(), "abc123");
        assert_eq!(sig.log_index().unwrap(), 42);
    }

    #[test]
    fn signature_converts_to_bundle() {
        let sig = Signature::new(sample_signature());
        let bundle = sig.to_bundle().unwrap();
        assert_eq!(bundle["rekorBundle"]["Payload"]["logIndex"], 42);
        assert!(bundle["Payload"].is_string());
    }

    #[test]
    fn payload_digest_mismatch_is_reported() {
        let sig = sample_signature();
        let pubkey = Path::new("unused");
        let result = verify_signature(&sig, "different-digest", pubkey);
        assert!(matches!(result, Err(SignatureError::SignatureMismatch(_))));
    }

    #[test]
    fn empty_signatures_fail_verification() {
        let pubkey = Path::new("unused");
        let result = verify_signatures(&[], "digest", pubkey);
        assert!(matches!(
            result,
            Err(SignatureError::SignatureVerificationError(_))
        ));
    }

    #[test]
    fn log_index_reducer_takes_the_maximum() {
        let sig = sample_signature();
        assert_eq!(get_log_index_from_signatures(&[sig.clone(), sig]), 42);
        assert_eq!(get_log_index_from_signatures(&[]), 0);
    }

    #[test]
    fn default_log_index_is_used_without_state_file() {
        assert_eq!(LAST_KNOWN_LOG_INDEX, 2023689270);
    }
}
