//! Types for the GitHub release manifest.
//!
//! The update check fetches the latest release of the Dangerzone repository from
//! GitHub (see `dangerzone/updater/releases.py`; there is no `manifest.py`
//! upstream). This module models the GitHub release JSON so that the update
//! check can report new application versions, changelogs and any container image
//! assets attached to the release.

use std::time::Duration;

use serde::Deserialize;

use crate::errors::UpdateError;

/// The endpoint that returns the latest GitHub release of Dangerzone.
pub const GH_RELEASE_URL: &str =
    "https://api.github.com/repos/freedomofpress/dangerzone/releases/latest";
/// The timeout of the manifest HTTP requests, in seconds.
pub const REQ_TIMEOUT_SECS: u64 = 15;

/// A GitHub release manifest.
///
/// Only the fields used by Dangerzone are modeled; unknown fields are ignored by
/// serde.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseManifest {
    /// The release tag, e.g. `v0.5.0`.
    pub tag_name: String,
    /// The release title.
    pub name: Option<String>,
    /// The release notes in Markdown.
    pub body: Option<String>,
    /// A link to the release page.
    pub html_url: Option<String>,
    /// The files attached to the release.
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

impl ReleaseManifest {
    /// The release version, without the leading `v`.
    pub fn version(&self) -> String {
        self.tag_name
            .strip_prefix('v')
            .unwrap_or(&self.tag_name)
            .to_string()
    }

    /// The changelog of the release, in Markdown.
    pub fn changelog(&self) -> Option<String> {
        self.body.clone()
    }

    /// Whether the release was fully parsed (has a version and a changelog).
    pub fn is_complete(&self) -> bool {
        !self.version().is_empty() && self.body.is_some()
    }
}

/// A file attached to a GitHub release.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    /// The asset filename.
    pub name: String,
    /// A link to download the asset.
    pub browser_download_url: String,
}

/// Downloads the latest release manifest from GitHub.
///
/// Corresponds to the request performed by `fetch_github_release_info()`. The
/// raw JSON text is returned so that it can be parsed by [`parse_manifest`].
pub fn download_manifest() -> Result<String, UpdateError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(REQ_TIMEOUT_SECS))
        .build();
    let response = agent
        .get(GH_RELEASE_URL)
        .set("User-Agent", "dangerzone-rust")
        .call()
        .map_err(|e| {
            UpdateError::Http(format!(
                "Encountered an error while checking {GH_RELEASE_URL}: {e}"
            ))
        })?;
    let status = response.status();
    if status != 200 {
        return Err(UpdateError::Http(format!(
            "Encountered an HTTP {status} error while checking {GH_RELEASE_URL}"
        )));
    }
    response.into_string().map_err(|e| {
        UpdateError::Http(format!(
            "Encountered an error while checking {GH_RELEASE_URL}: {e}"
        ))
    })
}

/// Parses the raw GitHub release manifest.
///
/// Corresponds to the JSON parsing performed by `fetch_github_release_info()`.
pub fn parse_manifest(json: &str) -> Result<ReleaseManifest, UpdateError> {
    let manifest: ReleaseManifest = serde_json::from_str(json).map_err(|e| {
        UpdateError::Http(format!(
            "Received a non-JSON response from {GH_RELEASE_URL}: {e}"
        ))
    })?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MANIFEST: &str = r#"{
        "tag_name": "v0.5.0",
        "name": "Dangerzone 0.5.0",
        "body": "Release **notes**",
        "html_url": "https://github.com/freedomofpress/dangerzone/releases/tag/v0.5.0",
        "assets": [
            {
                "name": "dangerzone-amd64.tar",
                "browser_download_url": "https://github.com/freedomofpress/dangerzone/releases/download/v0.5.0/dangerzone-amd64.tar"
            }
        ],
        "unknown_field": 42
    }"#;

    #[test]
    fn parses_a_release_manifest() {
        let manifest = parse_manifest(SAMPLE_MANIFEST).unwrap();
        assert_eq!(manifest.version(), "0.5.0");
        assert_eq!(manifest.changelog().as_deref(), Some("Release **notes**"));
        assert_eq!(manifest.assets.len(), 1);
        assert_eq!(manifest.assets[0].name, "dangerzone-amd64.tar");
        assert!(manifest.is_complete());
    }

    #[test]
    fn version_strips_the_leading_v() {
        let manifest = parse_manifest(SAMPLE_MANIFEST).unwrap();
        assert_eq!(manifest.tag_name, "v0.5.0");
        assert_eq!(manifest.version(), "0.5.0");
    }

    #[test]
    fn rejects_non_json_manifest() {
        assert!(parse_manifest("not json").is_err());
    }
}
