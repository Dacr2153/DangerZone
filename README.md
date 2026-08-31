# Dangerzone-RS

**A Rust-based document sanitizer that converts untrusted files into safe PDFs using sandboxed rasterization and reconstruction.**

Dangerzone-RS is a complete Rust rewrite of [Dangerzone](https://github.com/freedomofpress/dangerzone) by Freedom of the Press Foundation. It takes potentially malicious documents — PDFs, Office files, images, EPUBs — and converts them into **safe PDFs** by processing them inside a disposable sandbox, rendering every page to raw RGB pixels, and building a new PDF from scratch. Active content (scripts, macros, embedded objects, exploits) is destroyed in the process; only harmless pixel data survives.

The project is **local**, **offline-first**, **network-free in the sandbox**, and **fully open-source**.

---

## Table of Contents

- [How It Works](#how-it-works)
- [Features](#features)
- [Architecture](#architecture)
- [Tech Stack](#tech-stack)
- [Project Structure](#project-structure)
- [Supported Formats](#supported-formats)
- [Prerequisites](#prerequisites)
- [Installation](#installation)
- [Configuration](#configuration)
- [Usage](#usage)
- [CLI Reference](#cli-reference)
- [Environment Variables](#environment-variables)
- [Security Model](#security-model)
- [Sandbox Image](#sandbox-image)
- [Testing](#testing)
- [Packaging](#packaging)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [License](#license)

---

## How It Works

```
[Untrusted file] ──► [Host: dz-dangerzone] ──► [Sandbox: dz-convert] ──► [Host: dz-output] ──► [Safe PDF]
                           │                          │                          │
                           │                          ├─ Parses/renderers:        ├─ Builds PDF from scratch
                           │                          │  PDFium, image crate,     │  (no inherited content)
                           │                          │  LibreOffice, Calibre     │
                           │                          │                          ├─ Strips all metadata
                           │                          ├─ No network              │
                           │                          ├─ No host filesystem      ├─ Validates output
                           │                          ├─ Non-root user           │  (rejects JS, forms,
                           │                          ├─ Seccomp + AppArmor      │   embedded files, etc.)
                           │                          ├─ Resource limits         │
                           │                          └─ Destroyed after each    │
                           │                             conversion               │
                           │                                                       │
                           ├─ Never parses the original file on the host           │
                           ├─ Validates inputs (size, format)                      │
                           └─ Orchestrates startup tasks, provider selection       │
                                                                                    │
                                                                              [Safe PDF on disk]
```

**Key security boundary:** The original file never touches any parser running on the host. All dangerous parsing happens inside the disposable sandbox. The host only receives raw pixel buffers and reconstructs a known-safe PDF from them.

---

## Features

- **Sandboxed conversion** — Untrusted documents are processed in an ephemeral container (Podman/Docker) or Qubes disposable VM with no network, non-root user, read-only rootfs, seccomp, and AppArmor.
- **Format detection** — Multi-layer detection using magic bytes (`infer` crate), HWP header inspection, SVG XML parsing, ZIP container introspection, and file extension fallback.
- **PDF rasterization** — PDFium (pinned, SHA-256-verified `libpdfium.so`) renders PDF pages to RGB bitmaps at configurable DPI.
- **Office document support** — LibreOffice headless converts DOC/DOCX/XLS/XLSX/PPT/PPTX/ODT/ODS/ODP to PDF as an intermediate step before rasterization.
- **EPUB support** — Calibre's `ebook-convert` pipeline converts ebooks to PDF before rasterization.
- **Image support** — The `image` crate decodes PNG, JPEG, GIF, BMP, TIFF; `resvg` renders SVG to raster.
- **OCR** — Tesseract produces searchable PDF pages when `--ocr-lang` is specified. Multi-language support (100+ languages).
- **Output validation** — The reconstructed PDF is re-parsed and checked for forbidden features (JavaScript, embedded files, launch actions, forms, etc.).
- **Deterministic output** — The same input always produces byte-identical safe PDFs (no timestamps, no inherited metadata).
- **Signed updates** — Container images are verified against cosign signatures using a bundled Freedom of the Press Foundation public key.
- **Cross-platform** — Linux, macOS, and Windows support. Podman or Docker as container runtime.
- **GUI** — Preliminary native graphical interface using `egui`/`eframe` (drag-and-drop, file dialogs, settings persistence, progress logs).
- **CLI** — Full-featured command-line interface for scripting and automation.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│  dz-cli (dz-dangerzone)                                                │
│  CLI entry point: argument parsing, provider selection,                │
│  startup/shutdown task orchestration, conversion progress display      │
├─────────────────────────────────────────────────────────────────────────┤
│  dz-core                                                               │
│  Document model, DangerzoneCore orchestration, Settings persistence,   │
│  IsolationProvider trait, startup/shutdown task framework, error types  │
├─────────────────────────────────────────────────────────────────────────┤
│  dz-runtime                                                            │
│  Container provider (Podman/Docker), Qubes provider (qrexec),         │
│  Dummy provider (testing), Podman machine management,                  │
│  wire protocol parsing, seccomp/AppArmor profile management            │
├─────────────────────────────────────────────────────────────────────────┤
│  dz-output                                                             │
│  PDF reconstruction from RGB pixels (lopdf), Flate compression,        │
│  OCR page merging, metadata stripping, output validation               │
├─────────────────────────────────────────────────────────────────────────┤
│  dz-update                                                             │
│  Container image installation, cosign signature verification,          │
│  GitHub release checking, OCI registry interaction, air-gap support    │
├─────────────────────────────────────────────────────────────────────────┤
│  dz-converter (sandbox binary: dz-convert)                             │
│  Runs INSIDE the container. Format detection, PDFium rasterization,    │
│  LibreOffice/Calibre/Tesseract integration, wire protocol output       │
├─────────────────────────────────────────────────────────────────────────┤
│  dz-gui                                                                │
│  Native GUI (egui/eframe): document queue, settings, progress logs,    │
│  update checking, startup task display                                  │
└─────────────────────────────────────────────────────────────────────────┘
```

### Wire Protocol

The host and sandbox communicate exclusively through stdin/stdout pipes. No shared filesystem, no network.

**Standard mode (no OCR):**

```
stdin  → raw document bytes
stdout → [u16 BE page_count]
         For each page:
           [u16 BE width]
           [u16 BE height]
           [u8; width × height × 3]  (raw RGB pixel data)
```

**OCR mode (`--ocr-lang`):**

```
stdin  → raw document bytes
stdout → [u16 BE page_count]
         For each page:
           [u32 BE pdf_page_length]
           [u8; pdf_page_length]  (searchable single-page PDF from Tesseract)
```

**Error signaling:** Non-zero exit codes (offset by 128) map to `ConversionError` variants in `dz-converter::errors`.

### Conversion Limits

| Parameter | Value |
|-----------|-------|
| Max input size | 100 MiB |
| Max pages | 10,000 |
| Max page width | 10,000 px |
| Max page height | 10,000 px |
| Default DPI | 150 |
| Max OCR page size | 256 MiB |

---

## Tech Stack

| Component | Technology |
|-----------|-----------|
| Language | Rust (edition 2021, stable) |
| CLI framework | `clap` 4.6 (derive) |
| PDF parsing/writing | `lopdf` 0.34 |
| PDF rasterization | `pdfium-render` 0.9 (FFI to PDFium) |
| Image decoding | `image` 0.25 (PNG, JPEG, GIF, BMP, TIFF) |
| SVG rendering | `resvg` 0.48 |
| Office conversion | LibreOffice (headless) |
| Ebook conversion | Calibre (`ebook-convert`) |
| OCR | Tesseract |
| Compression | `flate2` (Zlib/Deflate) |
| File type detection | `infer` 0.22 |
| GUI framework | `egui`/`eframe` 0.28 |
| Native file dialogs | `rfd` 0.11 |
| Container runtime | Podman or Docker |
| Sandbox base image | Debian Bookworm (slim) |
| Signature verification | Cosign (bundled binary), Sigstore/Rekor transparency log |
| Serialization | `serde`/`serde_json` |
| Error handling | `thiserror` 2 |
| HTTP client | `ureq` 2 |
| Versioning | Semantic (`semver` 1) |

---

## Project Structure

```
dangerzone-rs/
├── Cargo.toml                     # Workspace root
├── Cargo.lock
├── crates/
│   ├── dz-core/                   # Core logic, document model, settings, traits
│   ├── dz-cli/                    # CLI binary (dz-dangerzone)
│   ├── dz-converter/              # Sandbox converter binary (dz-convert)
│   ├── dz-output/                 # Pixels → PDF reconstruction + validation
│   ├── dz-runtime/                # Isolation providers (Container, Qubes, Dummy)
│   ├── dz-update/                 # Signed update mechanism + container image management
│   └── dz-gui/                    # Native GUI (egui/eframe)
├── tests/                         # End-to-end integration tests + fixtures
│   ├── tests/
│   │   ├── cli.rs                 # CLI behavior tests
│   │   ├── conversion.rs          # Conversion pipeline tests (Dummy + Container)
│   │   └── common/mod.rs          # Test helpers (Runner, fixture_in_tempdir)
│   ├── fixtures/
│   │   ├── sample.pdf             # Test PDF fixture
│   │   └── sample.png             # Test PNG fixture
│   └── fuzz/fuzz_targets/         # Fuzz targets (placeholder)
├── sandbox/
│   ├── Containerfile              # Multi-stage Dockerfile for sandbox image
│   └── policies/
│       ├── seccomp.json           # Seccomp syscall allowlist
│       └── apparmor.profile       # AppArmor confinement profile
├── scripts/
│   ├── build-image.sh             # Build sandbox container image
│   └── sign-release.sh            # Release signing (placeholder)
├── share/
│   ├── version.txt                # Current version (0.1.5)
│   ├── image-name.txt             # Default container image name
│   ├── ocr-languages.json         # OCR language code → name mapping
│   ├── freedomofpress-dangerzone.pub  # Cosign public key for image verification
│   ├── rekor.pub                  # Rekor transparency log public key
│   ├── icons/                     # Application icons (placeholder)
│   └── translations/              # i18n files (placeholder)
├── packaging/
│   ├── linux/{deb,rpm}/           # Linux packaging (placeholders)
│   ├── macos/                     # macOS packaging (placeholder)
│   └── windows/                   # Windows packaging (placeholder)
├── docs/
│   └── architecture.md            # Detailed architecture documentation
├── .gitignore
├── .dockerignore
└── README.md
```

---

## Supported Formats

### Phase 1 — MVP

| Format | Extension(s) | Conversion Path |
|--------|-------------|-----------------|
| PDF | `.pdf` | PDFium → raster → PDF |
| PNG | `.png` | `image` crate → raster → PDF |
| JPEG | `.jpg`, `.jpeg` | `image` crate → raster → PDF |
| TIFF | `.tif`, `.tiff` | `image` crate → raster → PDF |
| GIF | `.gif` | `image` crate → raster → PDF |
| BMP | `.bmp` | `image` crate → raster → PDF |
| SVG | `.svg` | resvg → raster → PDF |

### Phase 2 — Office Formats

| Format | Extension(s) | Conversion Path |
|--------|-------------|-----------------|
| Word | `.doc`, `.docx` | LibreOffice → PDF → PDFium → raster → PDF |
| Excel | `.xls`, `.xlsx` | LibreOffice → PDF → PDFium → raster → PDF |
| PowerPoint | `.ppt`, `.pptx` | LibreOffice → PDF → PDFium → raster → PDF |
| OpenDocument | `.odt`, `.ods`, `.odp` | LibreOffice → PDF → PDFium → raster → PDF |

### Phase 3 — Additional Formats

| Format | Extension(s) | Conversion Path |
|--------|-------------|-----------------|
| EPUB | `.epub` | Calibre → PDF → PDFium → raster → PDF |
| HWP | `.hwp` | LibreOffice → PDF → PDFium → raster → PDF |

---

## Prerequisites

- **Rust** (stable) and **Cargo** — [rustup.rs](https://rustup.rs/)
- **Podman** (recommended) or **Docker** — running and accessible
- **Build tools** for the sandbox image: `curl`, `sha256sum`, `tar`

### Optional

- **LibreOffice** (for Office format support)
- **Calibre** (for EPUB support)
- **Tesseract** (for OCR)

> **Note:** LibreOffice, Calibre, and Tesseract are pre-installed inside the sandbox container image. They are only needed on the host for development or the Dummy provider.

---

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/freedomofpress/dangerzone-rs
cd dangerzone-rs

# Build all crates
cargo build --workspace --release

# Build the sandbox container image (requires Podman or Docker)
scripts/build-image.sh
```

The `build-image.sh` script:
1. Downloads the pinned PDFium shared library from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) (SHA-256 verified).
2. Stages it in `.cache/pdfium/`.
3. Builds the multi-stage `dangerzone-sandbox:latest` image.

#### build-image.sh Options

```bash
scripts/build-image.sh [OPTIONS]

Options:
  -n, --no-pdfium    Skip PDFium download; fail if not already staged.
  -a, --arg ARG      Extra build argument for the container runtime (repeatable).
  --runtime RUNTIME  Use 'podman' or 'docker' (default: podman).
  -h, --help         Show help.
```

### Binaries Produced

| Binary | Source Crate | Purpose |
|--------|-------------|---------|
| `dz-dangerzone` | `dz-cli` | Main CLI entry point |
| `dz-convert` | `dz-converter` | Sandbox converter (runs inside container) |
| `dz-podman` | `dz-runtime` | Podman machine management helper |
| `dz-update-image` | `dz-update` | Container image update management |
| `dz-gui` | `dz-gui` | Native GUI application |

---

## Configuration

### Settings File

Settings are persisted as JSON in the platform-appropriate config directory:

| Platform | Path |
|----------|------|
| Linux | `~/.config/dangerzone/settings.json` |
| macOS | `~/Library/Application Support/dangerzone/settings.json` |
| Windows | `%APPDATA%\dangerzone\settings.json` |

### Settings Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `save` | `bool` | `true` | Whether to save converted documents |
| `archive` | `bool` | `true` | Whether to archive originals in `unsafe/` subdirectory |
| `ocr` | `bool` | `true` | Whether OCR is enabled |
| `ocr_language` | `String` | `"English"` | Selected OCR language |
| `open` | `bool` | `true` | Whether to open the safe document after conversion |
| `open_app` | `Option<String>` | `None` | Custom app to open the safe document |
| `safe_extension` | `String` | `"-safe.pdf"` | Suffix for safe document filenames |
| `output_dir` | `Option<String>` | `None` | Custom output directory |
| `stop_other_podman_machines` | `String` | `"ask"` | Policy for conflicting Podman machines: `"ask"`, `"always"`, or `"never"` |
| `container_runtime` | `Option<String>` | `None` | Custom container runtime path |
| `updater_ask_before_download` | `bool` | `true` | Ask before downloading updates |
| `updater_check_all` | `Option<bool>` | `None` | Whether update checks are enabled |
| `updater_last_check` | `Option<i64>` | `None` | UNIX timestamp of last update check |
| `updater_latest_version` | `String` | (current) | Latest known version |
| `updater_latest_changelog` | `String` | `""` | Markdown changelog of latest version |
| `updater_remote_log_index` | `u32` | `0` | Last observed remote image log index |
| `updater_errors` | `u32` | `0` | Consecutive update check error count |

---

## Usage

### Basic Conversion

```bash
# Convert a single document
dangerzone document.pdf

# Convert multiple documents
dangerzone file1.pdf file2.docx image.png

# Specify output filename (single file only)
dangerzone --output-filename safe_output.pdf document.pdf

# Enable OCR with a specific language
dangerzone --ocr-lang "English" document.pdf

# Archive original documents in an `unsafe/` subdirectory
dangerzone --archive document.pdf
```

### Development Mode

```bash
# Build and run in dev mode with the Dummy provider (no container needed)
DANGERZONE_DEV=1 cargo run --bin dz-dangerzone -- --unsafe-dummy-conversion file.pdf

# Debug mode: enables sandbox logging
dangerzone --debug document.pdf
```

### Container Runtime Selection

```bash
# Use Docker instead of Podman
dangerzone --set-container-runtime docker

# Set a custom runtime path
dangerzone --set-container-runtime /usr/local/bin/podman

# Revert to the default runtime
dangerzone --set-container-runtime default
```

### GUI

```bash
# Launch the graphical interface
cargo run --bin dz-gui
```

---

## CLI Reference

```
dangerzone [OPTIONS] [FILENAMES]...
```

### Positional Arguments

| Argument | Description |
|----------|-------------|
| `FILENAMES...` | Documents to convert (at least one required) |

### Options

| Option | Description |
|--------|-------------|
| `--output-filename <VALUE>` | Output PDF filename (single file only) |
| `--ocr-lang <VALUE>` | OCR language (e.g., `"English"`, `"German"`) |
| `--archive` | Archive originals in `unsafe/` subdirectory |
| `--debug` | Enable debug logging from the sandbox |
| `--set-container-runtime <VALUE>` | Set container runtime (`docker`, `podman`, path, or `default`) |
| `--linger` | Keep Podman machine running after conversion |
| `--version` | Print version and exit |
| `--unsafe-dummy-conversion` | Use Dummy provider (hidden, dev only) |

---

## Environment Variables

### Development & Debug

| Variable | Description |
|----------|-------------|
| `DANGERZONE_DEV` | Set to `"1"` to enable development mode (Dummy provider, verbose errors, debug resource paths) |
| `DANGERZONE_PREFIX` | Override the install prefix for resource lookup (Linux, default: `/usr/local`) |

### Container Runtime

| Variable | Description |
|----------|-------------|
| `DANGERZONE_CONTAINER_RUNTIME` | Force `"podman"` or `"docker"` as the container runtime |
| `DANGERZONE_PODMAN` | Override the path to the `podman` binary |
| `DANGERZONE_IMAGE_NAME` | Override the container image name (default: `dangerzone-sandbox:latest`) |
| `DANGERZONE_CACHE_DIR` | Override the build cache directory (default: `.cache`) |

### Sandbox Converter

| Variable | Description |
|----------|-------------|
| `DANGERZONE_OCR_LANG` | Default OCR language when `--ocr-lang` is not passed |
| `DANGERZONE_LIBPDFIUM` | Override path to `libpdfium.so` (development/testing) |
| `DANGERZONE_TESSERACT` | Override path to the `tesseract` binary (testing) |
| `DANGERZONE_EBOOK_CONVERT` | Override path to `ebook-convert` (testing) |

### Security

| Variable | Description |
|----------|-------------|
| `DANGERZONE_BYPASS_SIGNATURE_VERIFICATION` | Set to `"1"` to skip container image signature verification |
| `DANGERZONE_BYPASS_SIG_CHECKS` | Dev-only: skip signature checks on bundled `container.tar` |

### Qubes OS

| Variable | Description |
|----------|-------------|
| `QUBES_CONVERSION` | Set to `"1"` in dev mode to force Qubes native conversion |
| `DANGERZONE_INSECURE_CONVERTER_PATH` | Path to the converter module for teleporting to a Qubes dispvm |

---

## Security Model

### Threat Boundary

The host machine is the trust boundary. Everything that parses the untrusted file runs inside the sandbox container. The host only consumes raw pixel buffers.

### Sandbox Security Layers

| Layer | Mechanism |
|-------|-----------|
| **Network isolation** | `--network=none` — no network access in the container |
| **User isolation** | Runs as unprivileged `dangerzone` user (UID 1000) |
| **Capability dropping** | `--cap-drop all` + only `SYS_CHROOT` added back |
| **Seccomp filtering** | Whitelist-based profile (~300 allowed syscalls), restricted `clone`/`ptrace` |
| **AppArmor** | Restricts filesystem access to `/opt/dangerzone`, `/usr`, `/lib`, `/home/dangerzone`, `/tmp` |
| **SELinux** | Label set to `container_engine_t` |
| **No new privileges** | `--security-opt no-new-privileges` prevents setuid escalation |
| **User namespace mapping** | `--userns nomap` (Podman ≥ 4.1) — no host UID mapping |
| **Read-only rootfs** | Base filesystem is read-only |
| **Ephemeral containers** | `--rm` — destroyed after each conversion |
| **No container logs** | `--log-driver none` — no data written to host |
| **Resource limits** | CPU, memory, disk, process count, timeouts enforced |

### Output Validation

The reconstructed PDF is validated post-construction by `dz-output::validator`:

- **Parseable:** Must parse as a valid PDF via `lopdf`.
- **Structure:** Must have a catalog, page tree, and at least one page.
- **No active content:** Rejects JavaScript, embedded files, launch actions, open actions, additional actions, forms, and dangerous action types (`GoToR`, `RichMediaExecute`, etc.).
- **Cycle detection:** Traverses the entire object graph to detect circular references.

### Metadata Sanitization

- No `CreationDate` or `ModDate` timestamps.
- No author, title, subject, keywords, or original metadata.
- Only `Producer` and `Creator` fields set to `"Dangerzone-RS {version}"`.
- Output is **deterministic** — same input always produces byte-identical PDF.

### Container Image Verification

- Cosign signatures are verified against the bundled Freedom of the Press Foundation public key.
- Rekor transparency log index is checked to prevent rollback.
- Verification runs before every conversion (bypassable in dev mode only).

---

## Sandbox Image

The sandbox container image is a multi-stage build based on Debian Bookworm:

### Builder Stage

- Rust toolchain compiles `dz-convert` binary with the `sandbox` feature.

### Runtime Stage

- **Base:** `debian:bookworm-slim`
- **Tools installed:** LibreOffice, Calibre, Tesseract (with English, German, French, Spanish, Portuguese, Italian, Dutch, Russian, Czech language data), Liberation and DejaVu fonts.
- **PDFium:** Pinned `libpdfium.so` from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries), SHA-256 verified.
- **User:** `dangerzone` (UID 1000, no login shell).
- **Environment:** `LD_LIBRARY_PATH=/opt/dangerzone/lib`, `OMP_THREAD_LIMIT=1`, `OMP_NUM_THREADS=1`.

### Building

```bash
scripts/build-image.sh

# Or with Docker
scripts/build-image.sh --runtime docker

# Skip PDFium download (must be pre-staged)
scripts/build-image.sh --no-pdfium
```

### Security Policies

| Policy | Location | Description |
|--------|----------|-------------|
| Seccomp | `sandbox/policies/seccomp.json` | Whitelist of ~300 syscalls, restricted `clone`/`clone3` bitmask, `PTRACE_TRACEME` only |
| AppArmor | `sandbox/policies/apparmor.profile` | Read-only access to system libraries, read-write to temp dirs and user home |

---

## Testing

### Unit and Integration Tests

```bash
# Run all tests (Dummy provider, no container required)
cargo test --workspace

# Run with verbose output
cargo test --workspace -- --nocapture
```

### Container Tests

Container end-to-end tests are opt-in because they require Podman and a locally built sandbox image:

```bash
# Run container integration tests
DANGERZONE_CONTAINER_TESTS=1 cargo test --workspace
```

### Test Coverage

| Test File | What It Tests |
|-----------|--------------|
| `tests/tests/cli.rs` | CLI argument parsing, suspicious option guard, output filenames, missing file handling |
| `tests/tests/conversion.rs` | Dummy conversion produces valid PDF, image inputs, output determinism, container conversion |
| `tests/tests/common/mod.rs` | Test harness: builds `dz-dangerzone` binary, runs subprocesses in temp dirs |

### Linting

```bash
# Clippy (workspace-wide)
cargo clippy --workspace --all-targets

# Formatting
cargo fmt --all -- --check
```

Workspace lints are configured in `Cargo.toml`:

```toml
[workspace.lints.rust]
missing_docs = "warn"
rust_2018_idioms = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = 10 }
perf = { level = "warn", priority = 9 }
```

---

## Packaging

Packaging support is planned but currently consists of placeholder directories:

| Platform | Location | Status |
|----------|----------|--------|
| Debian `.deb` | `packaging/linux/deb/` | Placeholder |
| RPM `.rpm` | `packaging/linux/rpm/` | Placeholder |
| macOS | `packaging/macos/` | Placeholder |
| Windows | `packaging/windows/` | Placeholder |

---

## Troubleshooting

### "No container image found"

Build the sandbox image:

```bash
scripts/build-image.sh
```

Or verify the image exists:

```bash
podman images dangerzone-sandbox
```

### "No container tech found"

Install Podman or Docker and ensure it is on your `PATH`.

### PDFium fails to load

The sandbox image includes a pinned `libpdfium.so`. For development outside the container:

```bash
# Set the PDFium library path manually
export DANGERZONE_LIBPDFIUM=/path/to/libpdfium.so
```

### macOS/Windows: Podman machine issues

Dangerzone manages a dedicated Podman machine named `dangerzone`. If another machine is running:

```bash
# List running Podman machines
podman machine list

# Stop conflicting machines (or set stop_other_podman_machines to "always")
dangerzone --linger document.pdf  # keeps the machine running after conversion
```

### Verbose debug output

```bash
dangerzone --debug document.pdf
DANGERZONE_DEV=1 dangerzone --debug document.pdf
```

---

## Contributing

1. Fork the repository.
2. Create a feature branch.
3. Make your changes following the existing code conventions.
4. Ensure `cargo test --workspace`, `cargo clippy --workspace`, and `cargo fmt --all -- --check` pass.
5. Submit a pull request.

### Code Conventions

- Use `thiserror` for error types.
- Prefer `log` macros over `println!` for diagnostic output.
- All public items should have doc comments (`missing_docs = "warn"`).
- Follow `clippy::all` and `clippy::perf` lint groups.
- Tests run in isolation using temporary directories; never modify the working directory.

---

## License

This project is licensed under the **MIT License**. See the [LICENSE](LICENSE) file for details.

---

## Further Reading

- [Architecture](./docs/architecture.md) — detailed design, component interactions, wire protocol, and threat model.
- [Dangerzone](https://github.com/freedomofpress/dangerzone) — the original Python implementation by Freedom of the Press Foundation.

---

**Dangerzone-RS** — because you shouldn't have to trust your documents.
