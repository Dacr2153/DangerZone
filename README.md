# Dangerzone-RS

**A Rust-based document sanitizer that converts untrusted files into safe PDFs using isolation and reconstruction.**

---

## What is Dangerzone-RS?

Dangerzone-RS is a complete rewrite of [Dangerzone](https://github.com/freedomofpress/dangerzone) in Rust. It takes potentially malicious documents (PDFs, Office files, images, etc.) and converts them into **safe PDFs** by processing them inside a sandbox, rendering them to raw pixels, and building a new PDF from scratch. This approach **removes active content** (scripts, macros, embedded objects) and **prevents exploits** from reaching your host.

The project is **local**, **offline-first**, and **fully open‑source**.

---

## How It Works (High‑Level)

```
[Input file]  →  [Host (Rust)]  →  [Sandbox (Container)]  →  [Pixels]  →  [New PDF]
                    │                      │
                    │                      ├── Parse/rendering using PDFium, the
                    │                      │   `image` crate, and LibreOffice.
                    │                      │── No network, no host filesystem,
                    │                      │   resource limits, seccomp.
                    │                      │── Destroys after each job.
                    │                      │
                    ├── Never parses original file.
                    ├── Validates inputs (size, format).
                    └── Reconstructs PDF from pixel data.
```

**Key security boundary:** The original file **never** touches the host’s own parsers – everything dangerous is inside the disposable sandbox.

---

## Core Components

| Component | Responsibility |
|-----------|----------------|
| **`dz-core`** | Document model, conversion logic, startup/shutdown tasks, security policies. |
| **`dz-cli`** | Command-line interface (`dz-dangerzone`). |
| **`dz-converter`** | Sandbox converter (`dz-convert`): format detection, rasterization via PDFium / LibreOffice / the `image` crate, wire protocol to the host. |
| **`dz-output`** | Reconstructs a new PDF from raster images; strips metadata; validates the output defensively. |
| **`dz-runtime`** | Pluggable isolation providers (container, Qubes, dummy). |
| **`dz-update`** | Container image management and updater stubs (real Sigstore verification deferred). |
| **`dz-gui`** | (Future) Graphical interface. |

---

## Supported Formats (Phased)

- **Phase 1 (MVP):** PDF, PNG, JPEG, TIFF.
- **Phase 2:** Office formats (DOCX, XLSX, PPTX, ODT, ODS, ODP) via LibreOffice.
- **Phase 3:** Older binary formats (DOC, XLS, PPT), EPUB, SVG.
- **Phase 4:** Additional image formats, HWP/HWPX.
- **Optional:** OCR (Tesseract) inside sandbox.

---

## Security Properties

- **No network access** in the sandbox – prevents exfiltration.
- **Non‑root user** inside the container.
- **Read‑only base filesystem**.
- **Drop all Linux capabilities** except those strictly required.
- **Seccomp filtering** to limit syscalls.
- **Resource limits** (CPU, memory, disk, processes, timeouts).
- **Ephemeral sandbox** – destroyed after each conversion.
- **Output validation** – generated PDF is verified against invariants (no JS, no embedded files, etc.).
- **Signed updates** for both the application and the container image.

---

## Quick Start (Development)

```bash
# Clone the repository
git clone https://github.com/your-org/dangerzone-rs
cd dangerzone-rs

# Build all crates
cargo build --workspace

# Build the sandbox container image (needs podman)
scripts/build-image.sh

# Run the CLI with the Dummy provider (no container required, dev only)
DANGERZONE_DEV=1 cargo run --bin dz-dangerzone -- --unsafe-dummy-conversion file.pdf

# Convert a real document through the sandbox
cargo run --bin dz-dangerzone -- file.pdf

# Run tests
cargo test --workspace
```

**Prerequisites:**
- Rust (stable) and Cargo.
- Podman (or Docker) installed and running.
- The container image must be built or pulled: `scripts/build-image.sh` (or `DANGERZONE_IMAGE_NAME=...` to override the image name).

---

## Project Structure

```
dangerzone-rs/
├── crates/               # Rust workspace members
│   ├── dz-core/          # Core logic
│   ├── dz-cli/           # CLI executable
│   ├── dz-converter/     # Sandbox converter (dz-convert binary)
│   ├── dz-output/        # Pixels → PDF
│   ├── dz-runtime/       # Isolation providers
│   ├── dz-update/        # Updater
│   └── dz-gui/           # GUI (future)
├── sandbox/              # Container definition and policies
├── tests/                # End-to-end integration tests + fixtures
├── scripts/              # Build and release helpers
└── docs/                 # Architecture
```

---

## License

This project is licensed under the **MIT License** (or choose a suitable license). See the `LICENSE` file for details.

---

## Current Status

- **Phases 0-4 complete**: workspace build, `dz-output`, sandbox converter, container image + policies, and end-to-end integration tests are all working. `cargo build/test/clippy --workspace` are clean (120+ tests).
- The container end-to-end test is opt-in: `DANGERZONE_CONTAINER_TESTS=1 cargo test --workspace`.
- The project is under active development; contributions are welcome.
- See `docs/architecture.md` for the design and `PlanDangerZone.md` for the roadmap.

---

## Why Rust?

- **Memory safety** without garbage collection.
- **Strong type system** helps enforce security boundaries.
- **Excellent error handling** and concurrency support.
- **Tooling** (cargo, clippy, fuzzing) that encourages robust code.
- **Easy integration** with system containers (Podman/Docker) via command execution.

---

## Further Reading

- [Architecture](./docs/architecture.md) – detailed design and component interactions.
- [Threat Model](./docs/threat-model.md) – security assumptions and attacker scenarios.
- [Sandbox Policy](./sandbox/policies/) – seccomp, AppArmor, and resource limits.

---

**Dangerzone-RS** – because you shouldn't have to trust your documents.