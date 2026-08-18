//! The sandbox-side document conversion entry point.
//!
//! The host streams the untrusted document on standard input; this binary
//! converts it to page pixel buffers (or, when `--ocr-lang` is passed, to
//! searchable PDF pages) and writes them on standard output using the
//! conversion wire protocol. Errors are printed to standard error and surfaced
//! to the host as the exit code of the corresponding conversion error class.

use std::io::Read;

use dz_converter::doc_to_pixels::{convert, MAX_INPUT_BYTES};
use dz_converter::errors::ConversionError;

/// The `--ocr-lang` command line flag.
const OCR_LANG_FLAG: &str = "--ocr-lang";
/// The environment variable that `--ocr-lang` falls back to.
const OCR_LANG_ENV: &str = "DANGERZONE_OCR_LANG";

fn main() {
    std::process::exit(run());
}

/// Extracts the OCR language from the arguments, honouring the
/// `DANGERZONE_OCR_LANG` environment variable as a fallback.
fn ocr_lang_from_args(args: &[String]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        if args[index] == OCR_LANG_FLAG {
            return args.get(index + 1).cloned();
        }
        index += 1;
    }
    std::env::var(OCR_LANG_ENV).ok()
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ocr_lang = ocr_lang_from_args(&args);

    let mut input = Vec::new();
    if let Err(error) = std::io::stdin()
        .lock()
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut input)
    {
        eprintln!("error reading the input document: {error}");
        return ConversionError::Unspecified.error_code() as i32;
    }

    let mut progress = |message: &str| eprintln!("{message}");
    match convert(
        input.as_slice(),
        std::io::stdout(),
        ocr_lang.as_deref(),
        &mut progress,
    ) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            error.error_code() as i32
        }
    }
}
