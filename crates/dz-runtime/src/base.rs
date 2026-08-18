//! The isolation provider abstraction.
//!
//! Corresponds to `dangerzone/isolation_provider/base.py`. An
//! [`IsolationProvider`] starts a conversion process that reads an untrusted
//! document on its standard input and writes the page pixel buffers on its
//! standard output. The provider then turns those buffers into a safe PDF.
//!
//! The original code uses PyMuPDF (`fitz`) to render pages and a
//! multiprocessing pool to run OCR in parallel. In this port pages are
//! assembled by the minimal PDF writer in [`dz_output::pdf`], and OCR is not
//! available.

use std::io::{self, Read};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, ExitStatus};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dz_converter::errors::{
    ConversionError, DEFAULT_DPI, INT_BYTES, MAX_PAGES, MAX_PAGE_HEIGHT, MAX_PAGE_WIDTH,
};
use dz_core::document::Document;
use dz_core::util;
use dz_output::pdf::{PdfDocument, PdfPage};

/// Seconds to wait for the conversion process to exit before treating the
/// failure as an unexpected conversion error.
pub const TIMEOUT_EXCEPTION: u64 = 15;
/// Seconds to wait for the conversion process to terminate gracefully.
pub const TIMEOUT_GRACE: u64 = 15;
/// Seconds to wait for the conversion process to exit after a forceful kill.
pub const TIMEOUT_FORCE: u64 = 5;
/// Maximum size, in bytes, of a single searchable page PDF returned by the
/// sandbox. Guards against a hostile sandbox claiming an absurd page size.
pub const MAX_OCR_PAGE_BYTES: u32 = 256 * 1024 * 1024;

/// A progress notification callback.
///
/// Mirrors the `progress_callback` of the Python code, invoked with the error
/// flag, the message text, and the completion percentage.
pub type ProgressCallback<'a> = &'a mut dyn FnMut(bool, &str, f64);

/// Errors raised while converting a document.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    /// The conversion process exited with a conversion error code.
    #[error(transparent)]
    Conversion(#[from] ConversionError),
    /// The conversion process exited before the expected output was produced.
    #[error("The process spawned for the conversion has exited early")]
    ConverterProc,
    /// A PDF assembly error.
    #[error(transparent)]
    Pdf(#[from] dz_output::pdf::PdfError),
    /// The assembled PDF failed post-write validation.
    #[error(transparent)]
    Validation(#[from] dz_output::validator::ValidationError),
    /// An I/O error while talking to the conversion process.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A document validation or manipulation error.
    #[error(transparent)]
    Document(#[from] dz_core::errors::DocumentFilenameError),
    /// A `podman` command failed.
    #[error(transparent)]
    Podman(#[from] crate::podman::errors::CommandError),
    /// A container image error.
    #[error(transparent)]
    Container(#[from] dz_core::errors::ContainerError),
}

/// A conversion subprocess and its standard streams.
///
/// Corresponds to `subprocess.Popen`. The child is always started in a new
/// session (process group), so that the whole group can be signaled later
/// without killing the controlling process.
pub struct ConversionProcess {
    /// The spawned child process.
    pub child: Child,
    /// The child's standard input, if it was configured as a pipe.
    pub stdin: Option<ChildStdin>,
    /// The child's standard output, if it was configured as a pipe.
    pub stdout: Option<ChildStdout>,
    /// The child's standard error, if it was configured as a pipe.
    pub stderr: Option<ChildStderr>,
}

impl ConversionProcess {
    /// Wraps a freshly spawned child, taking ownership of its streams.
    pub fn new(mut child: Child) -> Self {
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Self {
            child,
            stdin,
            stdout,
            stderr,
        }
    }

    /// The PID of the child process.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Checks whether the child has exited, without blocking.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Waits for the child to exit, up to the given timeout.
    ///
    /// Returns `Ok(None)` when the timeout expires and the child is still
    /// running, mirroring `subprocess.Popen.wait(timeout)` raising
    /// `TimeoutExpired`.
    pub fn wait_timeout(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(status));
            }
            if start.elapsed() >= timeout {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Reads exactly `size` bytes from a pipe, failing when the pipe ends early.
///
/// Corresponds to `read_bytes`.
fn read_bytes(reader: &mut impl Read, size: usize) -> Result<Vec<u8>, ConvertError> {
    let mut buf = vec![0u8; size];
    reader
        .read_exact(&mut buf)
        .map_err(|_| ConvertError::ConverterProc)?;
    Ok(buf)
}

/// Reads a big-endian integer from a pipe, mirroring `read_int`.
fn read_int(reader: &mut impl Read) -> Result<u16, ConvertError> {
    let buf = read_bytes(reader, INT_BYTES)?;
    Ok(u16::from_be_bytes([buf[0], buf[1]]))
}

/// Reads a big-endian `u32` from a pipe.
fn read_u32(reader: &mut impl Read) -> Result<u32, ConvertError> {
    let buf = read_bytes(reader, 4)?;
    Ok(u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]))
}

/// Reads all the captured bytes and returns a sanitized version of them.
///
/// Corresponds to `sanitize_debug_text`.
pub fn sanitize_debug_text(text: &[u8]) -> String {
    let untrusted_text = String::from_utf8_lossy(text);
    util::replace_control_chars(&untrusted_text, true)
}

/// Sends a signal to the process group of a conversion process.
///
/// Corresponds to `_signal_process_group`. The child is spawned in a new
/// session, so its PID equals its process-group ID.
#[cfg(unix)]
fn signal_process_group(p: &mut ConversionProcess, signo: libc::c_int) {
    // SAFETY: killpg with the child's pid (its process-group id, since it was
    // started in a new session) is a plain libc call.
    let result = unsafe { libc::killpg(p.child.id() as libc::pid_t, signo) };
    if result != 0 {
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            // If the process no longer exists, either when looking for the
            // process group (ESRCH) or when killing a process group that no
            // longer exists (EPERM), we can safely ignore the error.
            Some(libc::ESRCH) | Some(libc::EPERM) => {}
            _ => log::error!(
                "Unexpected error while sending signal {} to the document-to-pixels \
                 process group (PID: {})",
                signo,
                p.child.id()
            ),
        }
    }
}

/// Terminates a process group, mirroring `terminate_process_group`.
pub fn terminate_process_group(p: &mut ConversionProcess) {
    if cfg!(target_os = "windows") {
        let _ = p.child.kill();
    } else {
        #[cfg(unix)]
        signal_process_group(p, libc::SIGTERM);
    }
}

/// Forcefully kills a process group, mirroring `kill_process_group`.
pub fn kill_process_group(p: &mut ConversionProcess) {
    if cfg!(target_os = "windows") {
        let _ = p.child.kill();
    } else {
        #[cfg(unix)]
        signal_process_group(p, libc::SIGKILL);
    }
}

/// Stops a conversion process, or ensures it has exited.
///
/// This is the common part of `IsolationProvider::ensure_stop_doc_to_pixels_proc`,
/// extracted into a free function so that overrides (e.g. the container
/// provider) can run it before adding provider-specific checks. The
/// termination happens as gracefully as possible, and never blocks
/// indefinitely.
pub fn ensure_stop_common<P: IsolationProvider + ?Sized>(
    provider: &P,
    document: &Document,
    p: &mut ConversionProcess,
    timeout_grace: u64,
    timeout_force: u64,
) {
    // Check if the process completed.
    if p.try_wait().ok().flatten().is_some() {
        return;
    }

    // At this point, the process is still running. Terminate it gracefully.
    provider.terminate_doc_to_pixels_proc(document, p);
    if p.wait_timeout(Duration::from_secs(timeout_grace))
        .ok()
        .flatten()
        .is_none()
    {
        log::warn!(
            "Conversion process did not terminate gracefully after {timeout_grace} seconds. \
             Killing it forcefully..."
        );

        // Forcefully kill the running process.
        kill_process_group(p);
        if p.wait_timeout(Duration::from_secs(timeout_force))
            .ok()
            .flatten()
            .is_none()
        {
            log::warn!(
                "Conversion process did not terminate forcefully after {timeout_force} seconds. \
                 Resources may linger..."
            );
        }
    }
}

/// Configures a command to start in a new session, so that the whole process
/// group can be signaled later without killing the controlling process.
///
/// Mirrors `start_new_session=True`.
pub fn spawn_in_new_session(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        let _ = command;
    }
}

/// The exit code of a process status, mirrored to the exit codes used by the
/// conversion error classes.
fn exit_code_of(status: ExitStatus) -> u32 {
    match status.code() {
        Some(code) if code >= 0 => code as u32,
        // Processes killed by a signal report no exit code. The Python
        // implementation reports a negative code (`-signum`), which does not
        // map to any known conversion error; report a sentinel instead.
        _ => u32::MAX,
    }
}

/// Abstracts an isolation provider.
///
/// Corresponds to `IsolationProvider(ABC)`.
pub trait IsolationProvider {
    /// Whether this provider needs an installation step.
    fn requires_install(&self) -> bool;

    /// The maximum number of conversions that can run in parallel.
    fn get_max_parallel_conversions(&self) -> usize;

    /// Starts the document-to-pixels conversion process for a document.
    ///
    /// When `ocr_lang` is set, the sandbox is asked to return searchable PDF
    /// pages instead of raw pixel buffers, and `convert_with_proc` switches to
    /// the OCR wire protocol.
    fn start_doc_to_pixels_proc(
        &self,
        document: &Document,
        ocr_lang: Option<&str>,
    ) -> Result<ConversionProcess, ConvertError>;

    /// Terminates gracefully the process started for the doc-to-pixels phase.
    fn terminate_doc_to_pixels_proc(&self, document: &Document, p: &mut ConversionProcess);

    /// Whether debug output is requested for this provider.
    fn debug(&self) -> bool;

    /// Whether the provider's standard error should be captured for logging.
    fn should_capture_stderr(&self) -> bool {
        self.debug() || util::is_dev()
    }

    /// Converts a document.
    ///
    /// Corresponds to `IsolationProvider.convert`.
    fn convert(
        &self,
        document: &mut Document,
        ocr_lang: Option<&str>,
        progress_callback: ProgressCallback<'_>,
    ) {
        document.mark_as_converting();
        let result = self.doc_to_pixels_proc(document, ocr_lang, progress_callback);
        match result {
            Ok(()) => {
                document.mark_as_safe();
                if document.archive_after_conversion() {
                    if let Err(error) = document.archive() {
                        log::warn!("Failed to archive doc {}: {}", document.id(), error);
                    }
                }
            }
            Err(error) => {
                log::warn!(
                    "An exception occurred while converting document '{}': {}",
                    document.id(),
                    error
                );
                self.print_progress(&*document, true, &error.to_string(), 0.0, progress_callback);
                document.mark_as_failed();
            }
        }
    }

    /// Converts a byte array of RGB pixels into a PDF page.
    fn pixels_to_pdf_page(
        &self,
        untrusted_data: &[u8],
        untrusted_width: u32,
        untrusted_height: u32,
    ) -> PdfPage {
        dz_output::pdf::render_pdf_page(
            untrusted_data,
            untrusted_width,
            untrusted_height,
            DEFAULT_DPI,
        )
    }

    /// Consumes the output of a conversion process, assembling the safe PDF.
    ///
    /// Corresponds to `IsolationProvider.convert_with_proc`.
    fn convert_with_proc(
        &self,
        document: &Document,
        ocr_lang: Option<&str>,
        p: &mut ConversionProcess,
        progress_callback: ProgressCallback<'_>,
    ) -> Result<(), ConvertError> {
        // Write the content of the to-be-converted document to the stdin of
        // the conversion process, then close it to signal EOF.
        {
            let mut input = std::fs::File::open(document.input_filename()?)?;
            let mut stdin = p.stdin.take().ok_or(ConvertError::ConverterProc)?;
            if let Err(error) = io::copy(&mut input, &mut stdin) {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return Err(ConvertError::ConverterProc);
                }
                return Err(ConvertError::Io(error));
            }
            // Dropping `stdin` closes the pipe.
        }

        // And read the stdout, which should contain the pixel buffers.
        let mut stdout = p.stdout.take().ok_or(ConvertError::ConverterProc)?;

        let n_pages = read_int(&mut stdout)?;
        if n_pages == 0 || u32::from(n_pages) > MAX_PAGES {
            return Err(ConvertError::Conversion(ConversionError::MaxPages));
        }

        let mut safe_doc = PdfDocument::new();

        // When OCR is requested the sandbox runs Tesseract per page and sends
        // back searchable PDF pages (a different wire protocol); otherwise the
        // pages are received as raw pixel buffers.
        match ocr_lang {
            Some(ocr_lang) => self.convert_with_proc_ocr(
                document,
                ocr_lang,
                n_pages,
                &mut stdout,
                &mut safe_doc,
                progress_callback,
            )?,
            None => {
                let step = 100.0 / f64::from(n_pages);
                let mut percentage = 0.0;

                for page in 1..=n_pages {
                    let width = read_int(&mut stdout)?;
                    let height = read_int(&mut stdout)?;
                    if !(1..=u16::try_from(MAX_PAGE_WIDTH).unwrap_or(u16::MAX)).contains(&width) {
                        return Err(ConvertError::Conversion(ConversionError::MaxPageWidth));
                    }
                    if !(1..=u16::try_from(MAX_PAGE_HEIGHT).unwrap_or(u16::MAX)).contains(&height) {
                        return Err(ConvertError::Conversion(ConversionError::MaxPageHeight));
                    }

                    // Three color channels per pixel.
                    let num_pixels = u32::from(width) * u32::from(height) * 3;
                    let untrusted_pixels = read_bytes(&mut stdout, num_pixels as usize)?;

                    let page_pdf = self.pixels_to_pdf_page(
                        &untrusted_pixels,
                        u32::from(width),
                        u32::from(height),
                    );
                    safe_doc.insert_pdf(page_pdf);
                    percentage += step;
                    let text = format!("Converted page {page}/{n_pages} to PDF");
                    self.print_progress(document, false, &text, percentage, progress_callback);
                }
            }
        }

        // Ensure nothing else is read after all bitmaps are obtained.
        drop(stdout);

        // Saving it with a different name first, because a PDF writer may not
        // be able to handle non-Unicode characters.
        let sanitized_output = document.sanitized_output_filename()?;
        let serialized = safe_doc.to_bytes()?;
        // Defense in depth: re-parse the freshly written PDF and refuse to
        // publish it if it carries any feature that should not survive
        // sanitization. This guards against writer regressions.
        dz_output::validator::validate_pdf(&serialized)?;
        std::fs::write(&sanitized_output, serialized)?;
        std::fs::rename(&sanitized_output, document.output_filename()?)?;

        self.print_progress(
            document,
            false,
            "Successfully converted document",
            100.0,
            progress_callback,
        );
        Ok(())
    }

    /// Consumes the OCR wire protocol of a conversion process.
    ///
    /// In OCR mode the sandbox sends `page_count` (`u16`), followed for every
    /// page by its length (`u32`) and the searchable single-page PDF bytes.
    /// Each page is handed to a bounded worker pool that validates it, and the
    /// completed pages are drained in order, mirroring the
    /// `drain_ocr_futures`/`ProcessPoolExecutor` pattern of the original code.
    /// Bounding the in-flight pages keeps the host's memory usage flat while
    /// the (slow) Tesseract runs inside the sandbox.
    fn convert_with_proc_ocr(
        &self,
        document: &Document,
        _ocr_lang: &str,
        n_pages: u16,
        stdout: &mut ChildStdout,
        safe_doc: &mut PdfDocument,
        progress_callback: ProgressCallback<'_>,
    ) -> Result<(), ConvertError> {
        use std::collections::VecDeque;
        use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};

        type OcrResult = Result<Vec<u8>, String>;

        // One validator thread per two CPUs, like the upstream worker pool.
        let workers = std::thread::available_parallelism()
            .map(|count| count.get() / 2)
            .unwrap_or(1)
            .max(1);
        // A per-page result receiver, resolved by a validator thread. The
        // validated page bytes travel back through the channel, so a page is
        // only ever held once in memory.
        let mut ocr_futures: VecDeque<(u16, Receiver<OcrResult>)> = VecDeque::new();

        let outcome = std::thread::scope(|scope| {
            let (submit_tx, submit_rx) =
                std::sync::mpsc::channel::<(u16, Vec<u8>, std::sync::mpsc::SyncSender<OcrResult>)>(
                );
            // The validator threads share the receiver. It is not `Clone`, so
            // it is guarded behind a mutex; the lock is released as soon as a
            // page has been taken, so validation still runs concurrently.
            let submit_rx = std::sync::Arc::new(std::sync::Mutex::new(submit_rx));
            for _ in 0..workers {
                let submit_rx = submit_rx.clone();
                scope.spawn(move || loop {
                    let (_page, page_bytes, result_tx) = {
                        let guard = submit_rx.lock().unwrap();
                        match guard.recv() {
                            Ok(page) => page,
                            Err(_) => break,
                        }
                    };
                    // Validate that the sandbox produced a well-formed
                    // single-page PDF before it reaches the final document.
                    let result = match dz_output::pdf::ocr_pdf_page(&page_bytes) {
                        Ok(()) => Ok(page_bytes),
                        Err(error) => Err(error.to_string()),
                    };
                    let _ = result_tx.send(result);
                });
            }

            let mut ocr_page_num = 0u16;

            // Drains completed OCR pages from the front of the queue. When
            // `block_until_below` is set, blocks until fewer than that many
            // pages remain pending.
            let mut drain_ocr_futures =
                |block_until_below: Option<usize>,
                 ocr_futures: &mut VecDeque<(u16, Receiver<OcrResult>)>| {
                    loop {
                        if ocr_futures.is_empty() {
                            break;
                        }
                        if let Some(bound) = block_until_below {
                            if ocr_futures.len() <= bound {
                                break;
                            }
                        }
                        let (page, result_rx) = ocr_futures.pop_front().expect("checked above");
                        let outcome = match block_until_below {
                            Some(_) => match result_rx.recv() {
                                Ok(outcome) => outcome,
                                Err(_) => return Err(ConvertError::ConverterProc),
                            },
                            None => match result_rx.try_recv() {
                                Err(TryRecvError::Empty) => {
                                    ocr_futures.push_front((page, result_rx));
                                    break;
                                }
                                Ok(outcome) => outcome,
                                Err(_) => return Err(ConvertError::ConverterProc),
                            },
                        };
                        match outcome {
                            Ok(page_pdf) => {
                                safe_doc.insert_ocr_page(page_pdf)?;
                                ocr_page_num += 1;
                                let ocr_percentage =
                                    f64::from(ocr_page_num) / f64::from(n_pages) * 100.0;
                                let text = format!(
                                    "Converted page {ocr_page_num}/{n_pages} to searchable PDF"
                                );
                                self.print_progress(
                                    document,
                                    false,
                                    &text,
                                    ocr_percentage,
                                    progress_callback,
                                );
                            }
                            Err(message) => {
                                return Err(ConvertError::Conversion(
                                    ConversionError::unexpected_conversion(format!(
                                        "OCR failed for page {page}: {message}"
                                    )),
                                ));
                            }
                        }
                    }
                    Ok(())
                };

            for page in 1..=n_pages {
                // Block if too many pages are waiting for OCR, so the queue
                // never exceeds twice the number of workers.
                if ocr_futures.len() >= 2 * workers {
                    drain_ocr_futures(Some(workers), &mut ocr_futures)?;
                }

                // Consume each page of the sandbox's output.
                let len = read_u32(stdout)?;
                if len == 0 || len > MAX_OCR_PAGE_BYTES {
                    return Err(ConvertError::Conversion(
                        ConversionError::unexpected_conversion(format!(
                            "the sandbox reported an invalid searchable page size: {len}"
                        )),
                    ));
                }
                let page_bytes = read_bytes(stdout, len as usize)?;

                // Send the page to the validator pool...
                let (result_tx, result_rx) = sync_channel(0);
                submit_tx
                    .send((page, page_bytes, result_tx))
                    .map_err(|_| ConvertError::ConverterProc)?;
                ocr_futures.push_back((page, result_rx));

                // ... and drain any pages that have finished.
                drain_ocr_futures(None, &mut ocr_futures)?;
            }

            // Once all pages have been submitted, wait for the remaining ones.
            drain_ocr_futures(Some(0), &mut ocr_futures)?;

            // Dropping the sender lets the validator threads exit, and the
            // scope waits for them.
            drop(submit_tx);
            Ok(())
        });
        outcome
    }

    /// Prints a progress message to the log and forwards it to the callback.
    fn print_progress(
        &self,
        document: &Document,
        error: bool,
        text: &str,
        percentage: f64,
        progress_callback: ProgressCallback<'_>,
    ) {
        let message = format!("[doc {}] {}% {text}", document.id(), percentage as i64);
        if error {
            log::error!("{message}");
        } else {
            log::info!("{message}");
        }
        progress_callback(error, text, percentage);
    }

    /// Returns the conversion error associated with a process exit code.
    ///
    /// Corresponds to `get_proc_exception`.
    fn get_proc_exception(&self, p: &mut ConversionProcess, timeout: u64) -> ConversionError {
        match p.wait_timeout(Duration::from_secs(timeout)) {
            Ok(Some(status)) => {
                dz_converter::errors::exception_from_error_code(exit_code_of(status))
            }
            Ok(None) => ConversionError::unexpected_conversion(format!(
                "Encountered an I/O error during document to pixels conversion, but the \
                 conversion process is still running after {timeout} seconds (PID: {})",
                p.id()
            )),
            Err(_) => ConversionError::unexpected_conversion(format!(
                "Encountered an I/O error during document to pixels conversion, but the status \
                 of the conversion process is unknown (PID: {})",
                p.id()
            )),
        }
    }

    /// Stops the conversion process, or ensures it has exited.
    ///
    /// Corresponds to `ensure_stop_doc_to_pixels_proc`. The termination should
    /// happen as gracefully as possible, and should not block indefinitely.
    fn ensure_stop_doc_to_pixels_proc(
        &self,
        document: &Document,
        p: &mut ConversionProcess,
        timeout_grace: u64,
        timeout_force: u64,
    ) {
        ensure_stop_common(self, document, p, timeout_grace, timeout_force);
    }

    /// Starts a conversion process, runs the conversion, and then cleans up.
    ///
    /// Corresponds to the `doc_to_pixels_proc` context manager, with the
    /// default timeouts.
    fn doc_to_pixels_proc(
        &self,
        document: &Document,
        ocr_lang: Option<&str>,
        progress_callback: ProgressCallback<'_>,
    ) -> Result<(), ConvertError> {
        self.doc_to_pixels_proc_with_timeouts(
            document,
            ocr_lang,
            progress_callback,
            TIMEOUT_EXCEPTION,
            TIMEOUT_GRACE,
            TIMEOUT_FORCE,
        )
    }

    /// Starts a conversion process, runs the conversion, and then cleans up.
    ///
    /// Corresponds to the `doc_to_pixels_proc` context manager with explicit
    /// timeouts.
    fn doc_to_pixels_proc_with_timeouts(
        &self,
        document: &Document,
        ocr_lang: Option<&str>,
        progress_callback: ProgressCallback<'_>,
        timeout_exception: u64,
        timeout_grace: u64,
        timeout_force: u64,
    ) -> Result<(), ConvertError> {
        let mut process = self.start_doc_to_pixels_proc(document, ocr_lang)?;

        // Capture the process stderr in memory.
        let stderr = process.stderr.take();
        let stderr_log: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_thread = stderr.map(|mut stream| {
            let log = Arc::clone(&stderr_log);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => log.lock().unwrap().extend_from_slice(&buf[..n]),
                    }
                }
            })
        });

        let result = self.convert_with_proc(document, ocr_lang, &mut process, progress_callback);

        // If the conversion process exited early, map its exit code to a
        // proper conversion error.
        let result = match result {
            Err(ConvertError::ConverterProc) => {
                let exception = self.get_proc_exception(&mut process, timeout_exception);
                Err(ConvertError::Conversion(exception))
            }
            other => other,
        };

        self.ensure_stop_doc_to_pixels_proc(document, &mut process, timeout_grace, timeout_force);

        if let Some(stderr_thread) = stderr_thread {
            // Wait for the thread to complete, then log the captured output.
            let _ = stderr_thread.join();
            let debug_log = sanitize_debug_text(&stderr_log.lock().unwrap());
            log::info!(
                "Conversion output (doc to pixels)\n\
                 ----- DOC TO PIXELS LOG START -----\n\
                 {debug_log}\
                 ----- DOC TO PIXELS LOG END -----"
            );
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn read_int_decodes_big_endian() {
        let mut cursor = Cursor::new(vec![0x00, 0x02]);
        assert_eq!(read_int(&mut cursor).unwrap(), 2);
    }

    #[test]
    fn read_int_fails_on_short_input() {
        let mut cursor = Cursor::new(vec![0x00]);
        assert!(matches!(
            read_int(&mut cursor),
            Err(ConvertError::ConverterProc)
        ));
    }

    #[test]
    fn read_bytes_reads_exactly_requested_size() {
        let mut cursor = Cursor::new(vec![1, 2, 3, 4]);
        assert_eq!(read_bytes(&mut cursor, 4).unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn read_bytes_fails_on_truncated_input() {
        let mut cursor = Cursor::new(vec![1, 2]);
        assert!(matches!(
            read_bytes(&mut cursor, 3),
            Err(ConvertError::ConverterProc)
        ));
    }

    #[test]
    fn sanitize_debug_text_keeps_newlines() {
        let text = b"line 1\nline 2\x00";
        assert_eq!(sanitize_debug_text(text), "line 1\nline 2\u{FFFD}");
    }

    #[test]
    #[cfg(unix)]
    fn exit_code_maps_normal_codes() {
        use std::os::unix::process::ExitStatusExt;
        // Exit codes are read from the process, so use a success status.
        assert_eq!(exit_code_of(ExitStatus::from_raw(0)), 0);
    }
}
