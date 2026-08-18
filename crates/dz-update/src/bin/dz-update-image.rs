//! Entry point of the `dz-update-image` binary, which installs and verifies
//! the Dangerzone sandbox container image.

use std::process::ExitCode;

fn main() -> ExitCode {
    dz_update::cli::main()
}
