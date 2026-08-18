//! The dummy isolation provider, for testing only.
//!
//! Corresponds to `dangerzone/isolation_provider/dummy.py`. It spawns a
//! "do-nothing" converter that returns two solid-color pages, so conversions
//! can be exercised without a container runtime. Like the original, it refuses
//! to run outside of a development build.

use std::io::{Read, Write};
use std::process::Stdio;

use dz_core::document::Document;
use dz_core::errors::UnsafeIsolationProvider;

use crate::base::{
    spawn_in_new_session, terminate_process_group, ConversionProcess, ConvertError,
    IsolationProvider,
};

/// Whether the current process was spawned as the dummy conversion process.
const DUMMY_PROC_MARKER: &str = "--dangerzone-dummy-converter";

/// The dummy conversion protocol, reading from `reader` and writing to
/// `writer`. Consumes all of the input and writes two 9x9 solid-color pages.
fn dummy_script_with(reader: &mut impl Read, writer: &mut impl Write) {
    let mut sink = Vec::new();
    let _ = reader.read_to_end(&mut sink);

    let pages: u16 = 2;
    let width: u16 = 9;
    let height: u16 = 9;

    let _ = writer.write_all(&pages.to_be_bytes());
    for _ in 0..pages {
        let _ = writer.write_all(&width.to_be_bytes());
        let _ = writer.write_all(&height.to_be_bytes());
        let _ = writer.write_all(&[b'A'; 9 * 9 * 3]);
    }
    let _ = writer.flush();
}

/// Runs the dummy conversion protocol on the current process's stdin/stdout.
///
/// Corresponds to `dummy_script`.
pub fn dummy_script() {
    let stdin = std::io::stdin().lock();
    let mut reader = std::io::BufReader::new(stdin);
    let stdout = std::io::stdout().lock();
    let mut writer = std::io::BufWriter::new(stdout);
    dummy_script_with(&mut reader, &mut writer);
}

/// If the process was spawned as the dummy conversion process, runs it.
///
/// A binary that links this crate should call this function early in `main`,
/// so that the `Dummy` provider can spawn a sibling process.
pub fn maybe_run_dummy_converter() -> bool {
    if std::env::args().any(|arg| arg == DUMMY_PROC_MARKER) {
        dummy_script();
        true
    } else {
        false
    }
}

/// Dummy isolation provider (FOR TESTING ONLY).
///
/// A "do-nothing" converter: the sanitized files are the same as the input
/// files. Useful for testing without the need to use a container runtime.
pub struct Dummy;

impl Dummy {
    /// Creates a new dummy provider.
    ///
    /// Sanity check: refuses to run outside of a development build, mirroring
    /// `dangerzone_dev` gating the `UnsafeIsolationProvider`.
    pub fn new() -> Result<Self, UnsafeIsolationProvider> {
        if !dz_core::util::is_dev() {
            return Err(UnsafeIsolationProvider);
        }
        Ok(Self)
    }
}

impl IsolationProvider for Dummy {
    fn requires_install(&self) -> bool {
        false
    }

    fn start_doc_to_pixels_proc(
        &self,
        _document: &Document,
        _ocr_lang: Option<&str>,
    ) -> Result<ConversionProcess, ConvertError> {
        let exe = std::env::current_exe().map_err(ConvertError::Io)?;
        let stderr = if self.should_capture_stderr() {
            Stdio::piped()
        } else {
            Stdio::null()
        };
        let mut command = std::process::Command::new(exe);
        command
            .arg(DUMMY_PROC_MARKER)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr);
        spawn_in_new_session(&mut command);
        let child = command.spawn().map_err(ConvertError::Io)?;
        Ok(ConversionProcess::new(child))
    }

    fn terminate_doc_to_pixels_proc(&self, _document: &Document, p: &mut ConversionProcess) {
        terminate_process_group(p);
    }

    fn get_max_parallel_conversions(&self) -> usize {
        1
    }

    fn debug(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dz_converter::errors::INT_BYTES;
    use std::io::Cursor;

    #[test]
    fn dummy_script_writes_two_solid_pages() {
        let mut input = Cursor::new(vec![0u8; 64]);
        let mut output = Vec::new();
        dummy_script_with(&mut input, &mut output);
        assert_eq!(
            output.len(),
            INT_BYTES + 2 * (INT_BYTES + INT_BYTES + 9 * 9 * 3)
        );
        assert_eq!(output[..2], 2u16.to_be_bytes());
        assert_eq!(output[2..4], 9u16.to_be_bytes());
        assert_eq!(output[4..6], 9u16.to_be_bytes());
    }

    #[test]
    fn dummy_provider_is_rejected_outside_dev_mode() {
        std::env::remove_var("DANGERZONE_DEV");
        assert!(Dummy::new().is_err());
    }
}
