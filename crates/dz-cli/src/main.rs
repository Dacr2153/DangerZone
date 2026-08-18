//! The `dz-dangerzone` command-line interface.
//!
//! Corresponds to `dangerzone/cli/main.py`. This is the entry point of the
//! end-user conversion CLI: it parses the arguments, picks the isolation
//! provider, runs the startup and shutdown tasks, and converts the documents.

#![warn(missing_docs)]

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use dz_core::document::{Document, ARCHIVE_SUBDIR};
use dz_core::errors::TaskError;
use dz_core::logic::{DangerzoneCore, IsolationProvider};
use dz_core::shutdown::{ContainerStopTask, MachineStopTask, ShutdownLogic};
use dz_core::startup::{
    ContainerInstallTask, MachineInitTask, MachineStartTask, MachineStopOthersTask, StartupLogic,
    Task, UpdateCheckTask, WSLInstallTask,
};
use dz_core::util;

use dz_runtime::base::IsolationProvider as RuntimeIsolationProvider;
use dz_runtime::container::Container;
use dz_runtime::dummy::Dummy;
use dz_runtime::qubes::Qubes;

mod args;

/// The Dangerzone conversion command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "dangerzone",
    about = "Convert documents into safe PDFs",
    disable_version_flag = true
)]
struct Cli {
    /// The filename to write the safe PDF to. Defaults to the input filename
    /// ending with `-safe.pdf`. Only valid with a single input file.
    #[arg(long)]
    output_filename: Option<String>,

    /// The language to OCR with, defaults to none.
    #[arg(long)]
    ocr_lang: Option<String>,

    /// Archives the unsafe version in a subdirectory named 'unsafe'.
    #[arg(long)]
    archive: bool,

    /// Uses the unsafe dummy conversion, for testing only.
    #[arg(long = "unsafe-dummy-conversion", hide = true)]
    dummy_conversion: bool,

    /// Runs Dangerzone in debug mode, to get logs from the sandbox.
    #[arg(long)]
    debug: bool,

    /// The name or full path of the container runtime to use. Specify 'default'
    /// to revert to the default runtime for this OS.
    #[arg(long)]
    set_container_runtime: Option<String>,

    /// Does not stop the Podman machine after the conversions have completed.
    #[arg(long)]
    linger: bool,

    /// The documents to convert.
    #[arg(
        value_name = "FILENAMES",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    filenames: Vec<String>,
}

/// An isolation provider selected at runtime.
///
/// The original code branches between three provider classes. Since the core
/// is generic over its provider, this enum boxes the choices behind a single
/// type that also implements the core's `IsolationProvider` trait.
enum Provider {
    /// The dummy provider, only available in development builds.
    Dummy(Dummy),
    /// The Qubes OS provider.
    Qubes(Qubes),
    /// The container provider.
    Container(Container),
}

impl Provider {
    /// Whether this provider needs the Dangerzone sandbox installed.
    fn requires_install(&self) -> bool {
        match self {
            Provider::Dummy(provider) => provider.requires_install(),
            Provider::Qubes(provider) => provider.requires_install(),
            Provider::Container(provider) => provider.requires_install(),
        }
    }
}

impl IsolationProvider for Provider {
    fn convert(
        &self,
        document: &mut Document,
        ocr_lang: Option<&str>,
        stdout_callback: Option<&(dyn Fn(&str) + Sync)>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // The runtime providers print progress through the log and forward the
        // friendly message to the given callback. The CLI mirrors the upstream
        // progress line (`[doc <id>] <pct>% <text>`) on stdout, and prints
        // error progress to stderr.
        let doc_id = document.id().to_string();
        let mut progress = |error: bool, text: &str, percentage: f64| {
            let message = format!("[doc {doc_id}] {}% {text}", percentage as i64);
            if error {
                eprintln!("{}", util::replace_control_chars(&message, false));
            } else if let Some(callback) = stdout_callback {
                callback(&message);
            }
        };
        match self {
            Provider::Dummy(provider) => provider.convert(document, ocr_lang, &mut progress),
            Provider::Qubes(provider) => provider.convert(document, ocr_lang, &mut progress),
            Provider::Container(provider) => provider.convert(document, ocr_lang, &mut progress),
        }
        Ok(())
    }

    fn get_max_parallel_conversions(&self) -> usize {
        match self {
            Provider::Dummy(provider) => provider.get_max_parallel_conversions(),
            Provider::Qubes(provider) => provider.get_max_parallel_conversions(),
            Provider::Container(provider) => provider.get_max_parallel_conversions(),
        }
    }
}

/// Entry point of the `dz-dangerzone` binary.
fn main() -> ExitCode {
    // If this process was spawned as the dummy conversion process, run the
    // dummy converter and exit early.
    if dz_runtime::dummy::maybe_run_dummy_converter() {
        return ExitCode::SUCCESS;
    }

    // Check for suspicious options before clap parses the arguments, mirroring
    // the `parse_args` override of the original.
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(message) = args::check_suspicious_options(&raw_args) {
        println!("{message}");
        return ExitCode::FAILURE;
    }

    // The dangerzone version is read from a resource at runtime, which clap
    // cannot embed in the derive, so `--version` is handled here, mirroring
    // click's `%(version)s` template.
    if raw_args.iter().any(|arg| arg == "--version") {
        println!("{}", util::get_version());
        return ExitCode::SUCCESS;
    }

    let cli = Cli::parse();

    setup_logging();
    display_banner();

    // Validate the filenames, mirroring the click callbacks that run during
    // argument parsing.
    let filenames = match args::validate_input_filenames(&cli.filenames) {
        Ok(filenames) => filenames,
        Err(error) => return fail_document(error),
    };
    let output_filename = match &cli.output_filename {
        Some(value) => match args::validate_output_filename(value) {
            Ok(filename) => Some(filename),
            Err(error) => return fail_document(error),
        },
        None => None,
    };

    // Handle the container runtime override, which exits before any conversion
    // happens.
    if let Some(runtime) = &cli.set_container_runtime {
        let mut settings = dz_core::settings::write_settings();
        if runtime == "default" {
            match settings.unset_custom_runtime() {
                Ok(()) => println!(
                    "Instructed Dangerzone to use the default container runtime for this OS"
                ),
                Err(error) => {
                    println!("❌ {error}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            match settings.set_custom_runtime(runtime, true) {
                Ok(container_runtime) => println!(
                    "Set the settings container_runtime to {}",
                    container_runtime.display()
                ),
                Err(error) => {
                    println!("❌ {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    if filenames.is_empty() {
        println!("Missing argument 'FILENAMES...'");
        return ExitCode::FAILURE;
    }

    // Choose the isolation provider, mirroring the `dangerzone_dev` and
    // `is_qubes_native_conversion` gates.
    let provider = if util::is_dev() && cli.dummy_conversion {
        match Dummy::new() {
            Ok(dummy) => Provider::Dummy(dummy),
            // Dummy::new() only fails outside a development build, which the
            // guard above already excludes.
            Err(error) => {
                println!("{error}");
                return ExitCode::FAILURE;
            }
        }
    } else if dz_runtime::qubes::is_qubes_native_conversion() {
        Provider::Qubes(Qubes::new(cli.debug))
    } else {
        Provider::Container(Container::new(cli.debug))
    };
    let requires_install = provider.requires_install();

    let mut core = match DangerzoneCore::new(provider) {
        Ok(core) => core,
        Err(error) => {
            println!("❌ {error}");
            return ExitCode::FAILURE;
        }
    };

    // Add the documents to the core, honoring the single-file output
    // restriction of the original.
    match (filenames.len(), output_filename) {
        (1, Some(output)) => {
            let input = filenames[0].to_string_lossy();
            let output = output.to_string_lossy();
            match core.add_document_from_filename(&input, Some(&output), cli.archive) {
                Ok(()) => {}
                Err(error) => return fail_document(error),
            }
        }
        (len, Some(_)) if len > 1 => {
            println!("--output-filename can only be used with one input file.");
            return ExitCode::FAILURE;
        }
        _ => {
            for filename in &filenames {
                let input = filename.to_string_lossy();
                match core.add_document_from_filename(&input, None, cli.archive) {
                    Ok(()) => {}
                    Err(error) => return fail_document(error),
                }
            }
        }
    }

    // Validate the OCR language, if any. The CLI accepts the language *name*
    // ("English"), which is then mapped to the Tesseract code ("eng") that the
    // sandbox uses.
    let ocr_lang_code = match &cli.ocr_lang {
        None => None,
        Some(ocr_lang) => {
            let valid = core.ocr_languages().values().any(|name| name == ocr_lang);
            if !valid {
                println!("Invalid OCR language code. Valid language codes:");
                for (code, name) in core.ocr_languages() {
                    println!("{name}: {code}");
                }
                return ExitCode::FAILURE;
            }
            // The validation above guarantees the name is present.
            core.get_ocr_language_code(ocr_lang)
        }
    };

    // Build the startup tasks, mirroring the list of the original.
    let mut tasks: Vec<Box<dyn Task>> = Vec::new();
    if requires_install {
        let updater = dz_update::updater::Updater;
        tasks = vec![
            Box::new(WSLInstallTask),
            Box::new(MachineStopOthersTask),
            Box::new(MachineInitTask),
            Box::new(MachineStartTask),
            Box::new(UpdateCheckTask::new(Box::new(updater))),
            Box::new(ContainerInstallTask::new(Box::new(updater))),
        ];
    }
    match StartupLogic::new_startup(tasks, true).run() {
        Ok(()) => {}
        Err(TaskError::UpdaterDisabledNoContainer(_)) => {
            println!(
                "\nNo container image found. Please initialize Dangerzone by running:\n\n    \
                 dangerzone-image upgrade\n"
            );
            return ExitCode::FAILURE;
        }
        Err(error) => {
            println!("❌ {error}");
            return ExitCode::FAILURE;
        }
    }

    print_header("Converting document(s) to safe PDF");
    core.convert_documents(ocr_lang_code.as_deref(), Some(&print_progress));

    // Shut down the sandbox unless the user asked the machine to linger.
    // Shutdown failures are logged by the runner itself.
    if requires_install && !cli.linger {
        let tasks: Vec<Box<dyn Task>> =
            vec![Box::new(ContainerStopTask), Box::new(MachineStopTask)];
        let _ = ShutdownLogic::new_shutdown(tasks, true).run();
    }

    let documents_safe = core.get_safe_documents();
    let documents_failed = core.get_failed_documents();

    if !documents_safe.is_empty() {
        print_header("Safe PDF(s) created successfully");
        for document in documents_safe {
            let output = document.output_filename().unwrap_or_default();
            println!(
                "{}",
                util::replace_control_chars(&output.to_string_lossy(), false)
            );
        }
        if cli.archive {
            print_header(&format!(
                "Unsafe (original) documents moved to '{ARCHIVE_SUBDIR}' subdirectory"
            ));
        }
    }

    if !documents_failed.is_empty() {
        print_header("Failed to convert document(s)");
        for document in documents_failed {
            let input = document.input_filename().unwrap_or(Path::new(""));
            println!(
                "{}",
                util::replace_control_chars(&input.to_string_lossy(), false)
            );
        }
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Prints a document error and returns the failure exit code.
fn fail_document(error: dz_core::errors::DocumentFilenameError) -> ExitCode {
    println!("{error}");
    ExitCode::FAILURE
}

/// Prints a section header, mirroring `print_header` of the original.
fn print_header(text: &str) {
    println!();
    println!("{text}");
}

/// Prints a conversion progress line to stdout.
///
/// The line is already formatted as `[doc <id>] <pct>% <text>` by the core's
/// progress callback; control characters are stripped so a malicious converter
/// cannot inject terminal escape sequences.
fn print_progress(text: &str) {
    println!("{}", util::replace_control_chars(text, false));
}

/// Configures the logger, mirroring the `EndUserLoggingFormatter` of the
/// original: INFO lines are printed verbatim, and other levels are prefixed
/// with their level name.
fn setup_logging() {
    let mut builder = env_logger::Builder::new();
    if util::is_dev() {
        builder
            .filter_level(log::LevelFilter::Debug)
            .format(|buf, record| writeln!(buf, "[{:<5}] {}", record.level(), record.args()));
    } else {
        builder
            .filter_level(log::LevelFilter::Info)
            .format(|buf, record| {
                if record.level() == log::Level::Info {
                    writeln!(buf, "{}", record.args())
                } else {
                    writeln!(buf, "{} {}", record.level(), record.args())
                }
            });
    }
    // Ignore the error when a logger is already initialized (e.g. in tests).
    let _ = builder.try_init();
}

/// Prints the Dangerzone banner.
///
/// The original prints the banner with terminal colors (colorama). This port
/// prints the plain artwork, since color handling is left to the terminal.
fn display_banner() {
    const TOP: &str = "╭──────────────────────────╮";
    const BOTTOM: &str = "╰──────────────────────────╯";
    const ART: [&str; 11] = [
        "│           ▄██▄           │",
        "│          ██████          │",
        "│         ███▀▀▀██         │",
        "│        ███   ████        │",
        "│       ███   ██████       │",
        "│      ███   ▀▀▀▀████      │",
        "│     ███████  ▄██████     │",
        "│    ███████ ▄█████████    │",
        "│   ████████████████████   │",
        "│    ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀    │",
        "│                          │",
    ];

    println!("{TOP}");
    for line in ART {
        println!("{line}");
    }

    // Center the version line the same way the original does.
    let version = util::get_version();
    let text = format!("Dangerzone v{version}");
    let left = 15usize.saturating_sub(version.len() + 1) / 2;
    let mut right = left;
    if left + version.len() + 1 + right < 15 {
        right += 1;
    }
    println!("│{}{text}{}│", " ".repeat(left), " ".repeat(right));
    println!("│ https://dangerzone.rocks │");
    println!("{BOTTOM}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_provider_requires_install() {
        assert!(Provider::Container(Container::new(false)).requires_install());
    }

    #[test]
    fn qubes_provider_does_not_require_install() {
        assert!(!Provider::Qubes(Qubes::new(false)).requires_install());
    }

    #[test]
    fn providers_limit_parallel_conversions() {
        let container = Provider::Container(Container::new(false));
        let qubes = Provider::Qubes(Qubes::new(false));
        assert_eq!(container.get_max_parallel_conversions(), 1);
        assert_eq!(qubes.get_max_parallel_conversions(), 1);
    }
}
