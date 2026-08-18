//! Entry point of the `dz-podman` binary, which manages the Podman machine
//! used by the container isolation provider on macOS and Windows.

use std::process::ExitCode;

fn main() -> ExitCode {
    dz_runtime::podman::cli::main()
}
