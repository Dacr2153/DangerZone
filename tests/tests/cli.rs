//! End-to-end tests for the `dz-dangerzone` command-line interface.
//!
//! These tests run the real binary and assert on exit codes and output, and on
//! the files it produces. The Dummy provider (gated by `DANGERZONE_DEV=1`) is
//! used wherever a conversion is needed, so the tests do not require a
//! container runtime.

mod common;

use common::{fixture_in_tempdir, Runner};

/// `--version` prints the Dangerzone version and exits successfully.
#[test]
fn version_flag_prints_the_version() {
    let output = Runner::new().arg("--version").run();
    output.assert_code(0);
    assert_eq!(output.stdout.trim(), dz_core::util::get_version());
}

/// A CLI option that is also a file in the working directory is rejected.
///
/// An attacker-controlled file named like an option (e.g. `--archive`) could
/// otherwise be parsed as a flag. The guard must refuse to proceed.
#[test]
fn suspicious_option_guard_rejects_option_shadowed_by_a_file() {
    let tempdir = tempfile::tempdir().expect("failed to create temporary directory");
    let shadow_file = tempdir.path().join("--archive");
    std::fs::write(&shadow_file, b"").expect("failed to create a file named like an option");

    let output = Runner::new()
        .current_dir(tempdir.path())
        .arg("--archive")
        .run();
    output
        .assert_code(1)
        .assert_stdout_contains("Security: Detected CLI options");
}

/// A missing input file is reported before any conversion is attempted.
#[test]
fn missing_input_file_is_rejected() {
    let missing = tempfile::tempdir()
        .expect("failed to create temporary directory")
        .path()
        .join("does-not-exist.pdf");
    let output = Runner::new().arg(&missing).run();
    output
        .assert_code(1)
        .assert_stdout_contains("Input file not found");
}

/// The safe PDF defaults to `<stem>-safe.pdf` next to the input file.
#[test]
fn dummy_conversion_writes_default_output_filename() {
    let (_tempdir, input) = fixture_in_tempdir("sample.pdf");
    let output = Runner::new()
        .env("DANGERZONE_DEV", "1")
        .arg("--unsafe-dummy-conversion")
        .arg(&input)
        .run();
    output
        .assert_code(0)
        .assert_stdout_contains("Safe PDF(s) created successfully");

    let expected = input.with_file_name("sample-safe.pdf");
    assert!(
        expected.exists(),
        "expected {} to be created",
        expected.display()
    );
}

/// An explicit `--output-filename` is honored for a single input file.
#[test]
fn explicit_output_filename_is_honored() {
    let (_tempdir, input) = fixture_in_tempdir("sample.pdf");
    let out_dir = tempfile::tempdir().expect("failed to create temporary directory");
    let output_file = out_dir.path().join("custom.pdf");

    let output = Runner::new()
        .env("DANGERZONE_DEV", "1")
        .arg("--unsafe-dummy-conversion")
        .arg("--output-filename")
        .arg(&output_file)
        .arg(&input)
        .run();
    output.assert_code(0);

    assert!(
        output_file.exists(),
        "expected {} to be created",
        output_file.display()
    );
}
