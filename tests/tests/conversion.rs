//! End-to-end conversion tests.
//!
//! The Dummy provider exercises the full host-side pipeline (converter
//! subprocess, wire protocol, pixel-to-PDF reconstruction and validation)
//! without a container runtime. A second test drives the real container
//! sandbox, but only when the environment opts in (`DANGERZONE_CONTAINER_TESTS`)
//! and a podman runtime with the Dangerzone image is available.

mod common;

use std::process::Command;

use common::{fixture_in_tempdir, Runner};

/// Runs a dummy conversion of a fixture and returns the path of the safe PDF
/// alongside the temporary directory keeping it alive.
fn dummy_convert(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let (tempdir, input) = fixture_in_tempdir(name);
    let output = Runner::new()
        .env("DANGERZONE_DEV", "1")
        .arg("--unsafe-dummy-conversion")
        .arg(&input)
        .run();
    output.assert_code(0);
    let stem = input.file_stem().expect("fixture has a file stem");
    let safe = input.with_file_name(format!("{}-safe.pdf", stem.to_string_lossy()));
    (tempdir, safe)
}

/// A Dummy conversion of the PDF fixture yields a safe PDF that passes the
/// output validator.
#[test]
fn dummy_conversion_produces_valid_safe_pdf() {
    let (_tempdir, safe) = dummy_convert("sample.pdf");
    let bytes = std::fs::read(&safe).expect("safe PDF was not written");
    assert!(bytes.starts_with(b"%PDF"), "output is not a PDF");
    dz_output::validator::validate_pdf(&bytes).expect("safe PDF must pass the output validator");
}

/// A Dummy conversion of the PNG fixture yields a safe PDF.
#[test]
fn dummy_conversion_accepts_image_inputs() {
    let (_tempdir, safe) = dummy_convert("sample.png");
    let bytes = std::fs::read(&safe).expect("safe PDF was not written");
    dz_output::validator::validate_pdf(&bytes).expect("safe PDF must pass the output validator");
}

/// Running the same conversion twice must produce byte-identical output.
///
/// The safe PDF carries no timestamps and is authored from the rasterized
/// pages alone, so the bytes must be reproducible.
#[test]
fn dummy_conversion_output_is_deterministic() {
    let (_first, first_safe) = dummy_convert("sample.pdf");
    let (_second, second_safe) = dummy_convert("sample.pdf");
    let first = std::fs::read(first_safe).expect("first safe PDF was not written");
    let second = std::fs::read(second_safe).expect("second safe PDF was not written");
    assert_eq!(first, second, "safe PDF output must be deterministic");
}

/// Whether podman is available on the PATH.
fn podman_available() -> bool {
    Command::new("podman")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Whether the Dangerzone sandbox image is present locally.
fn sandbox_image_available() -> bool {
    let image = dz_runtime::container_utils::expected_image_name();
    Command::new("podman")
        .arg("image")
        .arg("exists")
        .arg(&image)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// End-to-end conversion through the real container sandbox.
///
/// Gated behind `DANGERZONE_CONTAINER_TESTS=1` because it needs podman plus a
/// locally built `dangerzone-sandbox` image. Skips cleanly otherwise.
#[test]
fn container_conversion_produces_valid_safe_pdf() {
    if std::env::var("DANGERZONE_CONTAINER_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping: set DANGERZONE_CONTAINER_TESTS=1 to run");
        return;
    }
    if !podman_available() {
        eprintln!("skipping: podman is not available");
        return;
    }
    if !sandbox_image_available() {
        eprintln!(
            "skipping: image {} is not installed (run scripts/build-image.sh)",
            dz_runtime::container_utils::expected_image_name()
        );
        return;
    }

    let (_tempdir, input) = fixture_in_tempdir("sample.pdf");
    let output = Runner::new().env("DANGERZONE_DEV", "0").arg(&input).run();
    output
        .assert_code(0)
        .assert_stdout_contains("Safe PDF(s) created successfully");

    let safe = input.with_file_name("sample-safe.pdf");
    let bytes = std::fs::read(&safe).expect("safe PDF was not written");
    assert!(bytes.starts_with(b"%PDF"), "output is not a PDF");
    dz_output::validator::validate_pdf(&bytes).expect("safe PDF must pass the output validator");
}
