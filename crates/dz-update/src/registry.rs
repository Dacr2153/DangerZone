//! Interface with the container registry.
//!
//! Corresponds to `dangerzone/updater/registry.py`. This client interacts with
//! container registries as defined by the OCI distribution spec:
//! <https://github.com/opencontainers/distribution-spec/blob/main/spec.md#endpoints>

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::errors::{RegistryError, UpdateError};

/// Media type of an OCI image index.
pub const IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
/// Media type of a Docker manifest list.
pub const IMAGE_LIST_MEDIA_TYPE: &str = "application/vnd.docker.distribution.manifest.list.v2+json";
/// The `Accept` header sent when querying manifest endpoints, mirroring the
/// original list.
pub const ACCEPT_MANIFESTS_HEADER: &str = concat!(
    "application/vnd.docker.distribution.manifest.v1+json,",
    "application/vnd.docker.distribution.manifest.v1+prettyjws,",
    "application/vnd.docker.distribution.manifest.v2+json,",
    "application/vnd.oci.image.manifest.v1+json,",
    "application/vnd.docker.distribution.manifest.list.v2+json,",
    "application/vnd.oci.image.index.v1+json",
);

/// A parsed container image location.
///
/// Corresponds to the `Image` dataclass of `registry.py`.
#[derive(Debug, Clone)]
pub struct Image {
    /// The registry host.
    pub registry: String,
    /// The namespace (owner) of the image.
    pub namespace: String,
    /// The image name.
    pub image_name: String,
    /// The tag, or `None` when only a digest is available.
    pub tag: Option<String>,
    /// The digest, without the `sha256:` prefix.
    pub digest: Option<String>,
}

impl Image {
    /// Serializes the image back to a string.
    ///
    /// The tag is only output when a digest is absent, or when the tag is
    /// `latest` and a digest is present, mirroring `Image.to_str()`.
    pub fn to_str(&self) -> String {
        let mut string = format!("{}/{}/{}", self.registry, self.namespace, self.image_name);
        if (self.tag.is_some() && self.digest.is_none())
            || (self.tag.as_deref() == Some("latest") && self.digest.is_some())
        {
            string.push(':');
            string.push_str(self.tag.as_deref().unwrap_or_default());
        }
        if let Some(digest) = &self.digest {
            string.push_str("@sha256:");
            string.push_str(digest);
        }
        string
    }
}

/// Parses a container image location into an [`Image`].
///
/// The accepted format is `registry/namespace/image[:tag][@sha256:digest]`,
/// mirroring the regular expression of `parse_image_location()`.
pub fn parse_image_location(input: &str) -> Result<Image, UpdateError> {
    let pattern = r"^(?P<registry>[a-zA-Z0-9.-]+)/(?P<namespace>[a-zA-Z0-9-]+)/(?P<image_name>[^:@]+)(?::(?P<tag>[a-zA-Z0-9.-]+))?(?:@(?P<digest>sha256:[a-zA-Z0-9]+))?$";
    let regex = Regex::new(pattern).expect("hardcoded image location regex is valid");
    let Some(captures) = regex.captures(input) else {
        return Err(UpdateError::Http("Malformed image location".to_string()));
    };
    Ok(Image {
        registry: captures["registry"].to_string(),
        namespace: captures["namespace"].to_string(),
        image_name: captures["image_name"].to_string(),
        tag: captures.name("tag").map(|m| m.as_str().to_string()),
        digest: captures.name("digest").map(|m| m.as_str().to_string()),
    })
}

/// Replaces the digest of an image location with `digest`, optionally removing
/// its tag.
pub fn replace_image_digest(
    image_str: &str,
    digest: &str,
    remove_tag: bool,
) -> Result<String, UpdateError> {
    let mut image = parse_image_location(image_str)?;
    image.digest = Some(digest.to_string());
    if remove_tag {
        image.tag = None;
    }
    Ok(image.to_str())
}

/// Fetches an anonymous bearer token for pulling from the registry.
fn get_auth_header(image: &Image) -> Result<String, UpdateError> {
    log::info!("Logging to the remote registry");
    let auth_url = format!("https://{}/token", image.registry);
    let scope = format!("repository:{}/{}:pull", image.namespace, image.image_name);
    let response = ureq::AgentBuilder::new()
        .build()
        .get(&auth_url)
        .query("service", &image.registry)
        .query("scope", &scope)
        .call()
        .map_err(|e| UpdateError::Registry(RegistryError::Http(e.to_string())))?;
    let text = response
        .into_string()
        .map_err(|e| UpdateError::Registry(RegistryError::Http(e.to_string())))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| UpdateError::Registry(RegistryError::Json(e)))?;
    let token = value.get("token").and_then(|v| v.as_str()).ok_or_else(|| {
        UpdateError::Registry(RegistryError::Http(
            "Registry token endpoint returned no token".to_string(),
        ))
    })?;
    Ok(format!("Bearer {token}"))
}

/// The base URL of the registry v2 API for an image.
fn url(image: &Image) -> String {
    format!(
        "https://{}/v2/{}/{}",
        image.registry, image.namespace, image.image_name
    )
}

/// Fetches the raw manifest bytes of an image.
fn get_manifest_raw(image_str: &str) -> Result<Vec<u8>, UpdateError> {
    let image = parse_image_location(image_str)?;
    let manifest_url = if let Some(digest) = &image.digest {
        format!("{}/manifests/{digest}", url(&image))
    } else {
        format!(
            "{}/manifests/{}",
            url(&image),
            image.tag.as_deref().unwrap_or("latest")
        )
    };
    let token = get_auth_header(&image)?;
    let response = ureq::AgentBuilder::new()
        .build()
        .get(&manifest_url)
        .set("Accept", ACCEPT_MANIFESTS_HEADER)
        .set("Authorization", &token)
        .call()
        .map_err(|e| UpdateError::Registry(RegistryError::Http(e.to_string())))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes)
        .map_err(|e| UpdateError::Registry(RegistryError::Http(e.to_string())))?;
    Ok(bytes)
}

/// Returns the digest of the manifest of the remote image.
///
/// Corresponds to `get_manifest_digest()`: the manifest is fetched with the
/// configured `Accept` header and hashed with SHA-256.
pub fn get_manifest_digest(image_str: &str) -> Result<String, UpdateError> {
    let content = get_manifest_raw(image_str)?;
    Ok(to_hex(Sha256::digest(&content).as_slice()))
}

/// Returns the digest of the manifest matching the given architecture, without
/// the `sha256:` prefix.
///
/// Corresponds to `get_digest_for_arch()`.
pub fn get_digest_for_arch(image_str: &str, architecture: &str) -> Result<String, UpdateError> {
    let content = get_manifest_raw(image_str)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&content).map_err(RegistryError::Json)?;
    let media_type = manifest
        .get("mediaType")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if media_type != IMAGE_LIST_MEDIA_TYPE && media_type != IMAGE_INDEX_MEDIA_TYPE {
        return Err(UpdateError::Registry(RegistryError::InvalidMultiArchImage));
    }
    let Some(manifests) = manifest.get("manifests").and_then(|v| v.as_array()) else {
        return Err(UpdateError::Registry(RegistryError::InvalidMultiArchImage));
    };
    let arch_manifests = manifests
        .iter()
        .filter(|manifest| {
            manifest
                .get("platform")
                .and_then(|platform| platform.get("architecture"))
                .and_then(|value| value.as_str())
                == Some(architecture)
        })
        .filter_map(|manifest| manifest.get("digest").and_then(|value| value.as_str()))
        .map(|digest| digest.replace("sha256:", ""))
        .collect::<Vec<_>>();
    if arch_manifests.is_empty() {
        return Err(UpdateError::Registry(RegistryError::ArchitectureNotFound));
    }
    Ok(arch_manifests[0].clone())
}

/// Formats a byte slice as a lowercase hexadecimal string.
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_image_location() {
        let image = parse_image_location("ghcr.io/freedomofpress/dangerzone/v1").unwrap();
        assert_eq!(image.registry, "ghcr.io");
        assert_eq!(image.namespace, "freedomofpress");
        assert_eq!(image.image_name, "dangerzone/v1");
        assert_eq!(image.tag, None);
        assert_eq!(image.digest, None);
    }

    #[test]
    fn parses_an_image_location_with_tag_and_digest() {
        let image =
            parse_image_location("docker.io/freedomofpress/dangerzone:v1@sha256:abc123").unwrap();
        assert_eq!(image.tag.as_deref(), Some("v1"));
        assert_eq!(image.digest.as_deref(), Some("sha256:abc123"));
    }

    #[test]
    fn rejects_a_malformed_image_location() {
        assert!(parse_image_location("dangerzone-sandbox:latest").is_err());
    }

    #[test]
    fn image_to_str_omits_tag_and_digest_combination() {
        let image = Image {
            registry: "ghcr.io".to_string(),
            namespace: "freedomofpress".to_string(),
            image_name: "dangerzone".to_string(),
            tag: Some("v1".to_string()),
            digest: Some("abc".to_string()),
        };
        assert_eq!(
            image.to_str(),
            "ghcr.io/freedomofpress/dangerzone@sha256:abc"
        );
    }

    #[test]
    fn image_to_str_keeps_tag_when_no_digest() {
        let image = Image {
            registry: "ghcr.io".to_string(),
            namespace: "freedomofpress".to_string(),
            image_name: "dangerzone".to_string(),
            tag: Some("v1".to_string()),
            digest: None,
        };
        assert_eq!(image.to_str(), "ghcr.io/freedomofpress/dangerzone:v1");
    }

    #[test]
    fn replaces_image_digest_and_removes_tag() {
        assert_eq!(
            replace_image_digest("ghcr.io/freedomofpress/dangerzone:v1", "deadbeef", true).unwrap(),
            "ghcr.io/freedomofpress/dangerzone@sha256:deadbeef"
        );
    }

    #[test]
    fn hex_encoding_is_lowercase() {
        assert_eq!(to_hex(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
