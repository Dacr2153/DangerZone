//! The Qubes OS isolation provider.
//!
//! Corresponds to `dangerzone/isolation_provider/qubes.py`. It performs the
//! conversion inside a disposable qube, started through a `qrexec` RPC call.

use std::io::Write;
use std::process::Stdio;

use dz_core::document::Document;

use crate::base::{spawn_in_new_session, ConversionProcess, ConvertError, IsolationProvider};

/// Environment variable pointing to the Dangerzone Python module, used to
/// teleport it into the disposable qube in development mode.
const INSECURE_CONVERTER_PATH_ENV: &str = "DANGERZONE_INSECURE_CONVERTER_PATH";

/// The Qubes isolation provider.
pub struct Qubes {
    debug: bool,
}

impl Qubes {
    /// Creates a new Qubes provider.
    pub fn new(debug: bool) -> Self {
        Self { debug }
    }

    /// Sends the Dangerzone module to another qube, as a zipfile.
    ///
    /// Corresponds to `teleport_dz_module`. The original bundles the Python
    /// module into a zipfile and sends its size followed by its bytes. This
    /// port does not ship a Python module to transfer, so an empty bundle is
    /// sent.
    fn teleport_dz_module(&self, _conv_path: &str, wpipe: &mut impl Write) {
        log::warn!(
            "Teleporting the dangerzone module is not supported in this port; \
             sending an empty module bundle"
        );
        let size_bytes = 0u32.to_be_bytes();
        let _ = wpipe.write_all(&size_bytes);
    }
}

impl IsolationProvider for Qubes {
    fn requires_install(&self) -> bool {
        false
    }

    fn get_max_parallel_conversions(&self) -> usize {
        1
    }

    fn start_doc_to_pixels_proc(
        &self,
        _document: &Document,
        _ocr_lang: Option<&str>,
    ) -> Result<ConversionProcess, ConvertError> {
        let conv_mod_path = std::env::var(INSECURE_CONVERTER_PATH_ENV).ok();
        let dev_mode = dz_core::util::is_dev() && conv_mod_path.is_some();

        let (qrexec_policy, stderr) = if dev_mode {
            // Use the dz.ConvertDev RPC call instead, if we are in development
            // mode. Basically, the change is that we also transfer the
            // necessary Python code as a zipfile, before sending the doc that
            // the user requested.
            ("dz.ConvertDev", Stdio::piped())
        } else {
            ("dz.Convert", Stdio::null())
        };

        let mut command = std::process::Command::new("/usr/bin/qrexec-client-vm");
        command
            .args(["@dispvm:dz-dvm", qrexec_policy])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr);
        spawn_in_new_session(&mut command);
        let child = command.spawn().map_err(ConvertError::Io)?;
        let mut process = ConversionProcess::new(child);

        if dev_mode {
            // Send the dangerzone module first.
            if let Some(conv_mod_path) = conv_mod_path {
                if let Some(stdin) = process.stdin.as_mut() {
                    self.teleport_dz_module(&conv_mod_path, stdin);
                }
            }
        }

        Ok(process)
    }

    fn terminate_doc_to_pixels_proc(&self, _document: &Document, p: &mut ConversionProcess) {
        // Qubes does not offer a way out of the box to terminate disposable
        // Qubes from domU. Our best bet is to close the standard streams of
        // the process, and hope that the disposable qube will attempt to
        // read/write to them, and thus receive an EOF.
        //
        // Note that we don't close the stderr stream because we want to read
        // debug logs from it.
        p.stdin.take();
        p.stdout.take();
    }

    fn debug(&self) -> bool {
        self.debug
    }
}

/// Returns `true` if the conversion should be run using Qubes OS's disposable
/// VMs, and `false` if not.
///
/// Corresponds to `is_qubes_native_conversion`.
pub fn is_qubes_native_conversion() -> bool {
    if std::path::Path::new("/usr/share/qubes/marker-vm").exists() {
        if dz_core::util::is_dev() {
            return std::env::var("QUBES_CONVERSION").as_deref() == Ok("1");
        }

        // If Dangerzone is installed, check if the container image was
        // shipped. This disambiguates if it is running a Qubes-targeted build
        // or not (Qubes-specific builds don't ship the container image).
        !crate::updater::signatures::is_container_tar_bundled()
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_conversion_requires_the_qubes_marker() {
        // On a non-Qubes system the marker file does not exist, so the
        // conversion is never native.
        if !std::path::Path::new("/usr/share/qubes/marker-vm").exists() {
            assert!(!is_qubes_native_conversion());
        }
    }
}
