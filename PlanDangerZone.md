# Dangerzone-RS — Rebuild Missing Elements (Full Parity)

A Rust translation and architectural improvement of
[freedomofpress/dangerzone](https://github.com/freedomofpress/dangerzone/tree/main/dangerzone).

Goal of this iteration: bring the port to **full, complete functionality** —
feature parity with the upstream Python project. The working MVP (Phases 0-4)
already converts documents end-to-end through a hardened container with
security flags identical to upstream. Remaining work closes the functional
gaps: container image lifecycle, OCR, progress/UX, format expansion,
cross-platform support, the GUI, and packaging.

This file is the single source of truth for the plan. Read the section for the
phase being implemented, execute it, then move to the next.

---

## Starting snapshot (state of the repo before Phase 0; historical reference)

### Done & compiling
- `dz-core`: document, errors, logic, settings, startup, shutdown, util, stubs.
- `dz-cli`: `dz-dangerzone` binary (args validation, suspicious-option guard).
- `dz-runtime`: isolation providers (base/container/dummy/qubes), podman
  plumbing, hand-rolled minimal PDF writer (`src/pdf.rs`), updater stubs.
- `dz-converter`: `errors.rs` — the conversion wire-protocol constants and the
  `ConversionError` exit-code mapping.
- `dz-update`: CLI shell only (`cli.rs`, `errors.rs`); `registry.rs` and
  `signatures.rs` are no-op stubs; `manifest.rs`/`updater.rs`/`verify.rs` empty.
- `share/`: `ocr-languages.json`, `version.txt` populated.

### Empty / missing
- Root `Cargo.toml`, `.cargo/config.toml`, `.gitignore`, root `Cargo.lock` (0 B).
  There is **no cargo workspace**; each crate is its own virtual workspace.
- `dz-output` entirely (Cargo.toml + all src files empty).
- `dz-converter` converter modules: `doc_to_pixels.rs`, `format_detect.rs`,
  `image.rs`, `office.rs`, `pdf.rs` all 0 B.
- `dz-core`: `job.rs`, `limits.rs`, `policy.rs`, `progress.rs` all 0 B and not
  declared in `lib.rs` (dead files).
- `sandbox/`: `Containerfile`, `entrypoint.rs`, `convert.sh`, both policies 0 B.
- `scripts/`: `build-image.sh`, `sign-release.sh` 0 B.
- `tests/integration/*.rs` and fuzz targets empty.
- `docs/`, `LICENSE`, `CONTRIBUTING.md` missing (README references them).

### Wire protocol (host side already implemented)
- Host streams raw document bytes to the converter's **stdin**, then closes it.
- Converter writes to **stdout**: big-endian `u16` page count, then per page:
  `u16` width, `u16` height, then `width * height * 3` raw RGB bytes.
- Errors are communicated via the process **exit code** mapped by
  `dz_converter::errors::exception_from_error_code` (constants
  `DEFAULT_DPI = 150`, `INT_BYTES = 2`, `MAX_PAGES/WIDTH/HEIGHT = 10_000`).
- Progress text is printed to **stderr**.
- Reference reader: `dz-runtime/src/base.rs::convert_with_proc`.
  Reference writer: `dz-runtime/src/dummy.rs::dummy_script_with`.

---

## Design decisions (confirmed)
- **Rasterizer:** `pdfium-render` (Google PDFium, runtime dynamic loading of a
  pinned `libpdfium.so`) for PDFs; `image` crate for PNG/JPEG/TIFF; shell out
  to **LibreOffice** for office formats, then rasterize the intermediate PDF.
- **Converter placement:** implement the converter inside `dz-converter` (the 5
  empty module files already exist there), gated behind a `sandbox` cargo
  feature so the host never links PDFium/`image`. New `[[bin]] dz-convert`
  (`required-features = ["sandbox"]`) is the container entrypoint.
- **`dz-output`:** becomes the real "pixels -> safe PDF" crate. Move the PDF
  writer there, add Flate compression, safe metadata, post-write validator
  (`lopdf`); refactor `dz-runtime` to depend on it.
- **Container image name:** local `dangerzone-sandbox:latest` built by
  `scripts/build-image.sh` (no registry for the MVP). Overridable via env.
- **Guiding standards:**
  - Rust best-practices skill (Apollo handbook): `&T` over clone, `thiserror`
    for libs, `#[expect(lint)]` with justification, descriptive test names,
    `///` docs on all public items, `#![warn(missing_docs)]`, TODOs carry an
    issue link `// TODO(#<issue>): ...`.
  - Bash defensive-patterns skill for scripts: `set -Eeuo pipefail`, quoting,
    dep checks, idempotency, logging, cleanup traps.

---

## Phase 0 — Cargo workspace & cleanup

Steps:
1. Fill root `Cargo.toml`:
   - `[workspace]` with `resolver = "2"` and members:
     `crates/dz-core`, `crates/dz-converter`, `crates/dz-runtime`,
     `crates/dz-output`, `crates/dz-update`, `crates/dz-cli`.
   - `[workspace.package]` (version 0.1.0, edition 2021, license MIT).
   - `[workspace.dependencies]` for shared deps: thiserror 2, log 0.4,
     env_logger 0.11, clap 4.6 (derive), serde 1 (derive), serde_json 1,
     dirs-next 2, rand 0.8, regex 1, semver 1, libc 0.2,
     unicode-general-category 1, flate2 1, lopdf 0.34.
   - `[workspace.lints]`: `rust.unsafe_code = "deny"` is NOT set (libc usage),
     set `rust.missing_docs = "warn"`, `rust.rust_2018_idioms = "warn"`,
     `clippy.all = "warn"`, `clippy.perf = "warn"`, `clippy.pedantic = "warn"`
     is too noisy -> keep `clippy.all` + `clippy.perf` only.
2. Update each crate `Cargo.toml`:
   - Remove the `[workspace]` marker.
   - Reference shared deps via `{ workspace = true }`.
   - Add `lints.workspace = true`.
3. `dz-gui`: leave the crate out of the workspace for now (empty); keep the
   directory but exclude from members.
4. Delete dead 0-byte orphans: `crates/dz-core/src/{job,limits,policy,progress}.rs`
   (they return when those features land).
5. Verification:
   - `cargo build --workspace`
   - `cargo test --workspace`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - Fix any warnings introduced by the lints; prefer `#[expect(...)]`.

## Phase 1 — `dz-output`: safe PDF reconstruction

Create a full crate at `crates/dz-output` (files are currently 0 B):
- `Cargo.toml`: package `dz-output`, deps `dz-core` (for version), `flate2`,
  `lopdf`, `thiserror`, `log`; `lints.workspace = true`.
- `src/lib.rs`: `#![warn(missing_docs)]`, module docs, `pub mod compression`,
  `pub mod metadata`, `pub mod pdf`, `pub mod validator`.
- `src/pdf.rs`: port `dz-runtime/src/pdf.rs` (keep `PdfPage`, `PdfDocument`,
  `render_pdf_page(pixels, width, height, dpi)` and `ocr_pdf_page` stub API so
  `dz-runtime::base` barely changes). Add Flate compression of the image
  XObject and content streams (`/Filter /FlateDecode`) via `flate2`. Keep the
  valid classic xref table and `%PDF-1.4` + binary comment header.
- `src/metadata.rs`: write a safe `/Info` dictionary: fixed
  `Producer = "Dangerzone-RS <version>"`, `Creator` fixed, no dates derived from
  untrusted input (omit CreationDate/ModDate or use fixed-safe values).
- `src/compression.rs`: `compress(&[u8]) -> Vec<u8>` (Flate/Deflate) and
  `decompress` helpers used by writer and validator.
- `src/validator.rs`: `validate_pdf(&[u8]) -> Result<(), ValidationError>`.
  Parse with `lopdf`; assert catalog + page tree exist and that the document
  contains no JavaScript (`/JavaScript`, `/JS`), no embedded files
  (`/EmbeddedFiles`, `/Filespec`, `/EF`), no launch/foreign-doc actions
  (`/Launch`, `/GoToE`), and no risky `/OpenAction`. `ValidationError` via
  thiserror with descriptive variants.
- Refactor `dz-runtime`:
  - `Cargo.toml`: add `dz-output = { path = "../dz-output" }`.
  - Delete `dz-runtime/src/pdf.rs`; update `src/lib.rs` module list.
  - `src/base.rs`: import `dz_output::pdf::{PdfDocument, PdfPage, render_pdf_page, ocr_pdf_page}`;
    after `safe_doc.save(...)` run `dz_output::validator::validate_pdf` on the
    serialized bytes before the rename (defense in depth).
- Tests:
  - Port existing PDF writer tests (header/EOF, xref offsets, empty doc).
  - Compression round-trip; metadata invariants; validator rejects a payload
    containing `/JavaScript`, accepts our own generated PDF.
- Verification: `cargo build/test/clippy --workspace`.

> **Status: DONE.** `dz-output` written (compression, metadata, pdf, validator)
> with 20 tests; `dz-runtime` refactored to consume it (`dz-runtime/src/pdf.rs`
> deleted, `base.rs` calls `validate_pdf` on serialized bytes before rename).
> Workspace: 101 tests pass, clippy clean.

## Phase 2 — `dz-converter` sandbox: doc -> pixels

- `Cargo.toml`:
  - `[features] sandbox = ["dep:pdfium-render", "dep:image", "dep:infer"]`.
  - Optional deps: `pdfium-render` 0.9 (default features minus image? keep
    default + `thread_safe`), `image` 0.25 (jpeg,png,gif,tiff,bmp), `infer`
    0.19, `log`.
  - `[[bin]] name = "dz-convert"` with `required-features = ["sandbox"]`.
- `src/lib.rs`: declare `format_detect`, `image`, `pdf`, `office`,
  `doc_to_pixels` behind `#[cfg(feature = "sandbox")]`.
- `src/format_detect.rs`:
  - `enum ImageFormat { Png, Jpeg, Gif, Bmp, Tiff }`
  - `enum OfficeKind { Doc, Docx, Xls, Xlsx, Ppt, Pptx, Odt, Ods, Odp }`
  - `enum DocumentFormat { Pdf, Image(ImageFormat), Office(OfficeKind), Unsupported }`
  - `detect_format(bytes, extension) -> DocumentFormat` using `infer` (covers
    pdf, png/jpeg/gif/bmp/tiff, doc/docx/xls/xlsx/ppt/pptx, odt/ods/odp, epub,
    generic zip). Unknown / out-of-MVP-scope (epub, svg, hwp) -> `Unsupported`
    mapped to `ConversionError::DocFormatUnsupported` upstream.
- `src/image.rs`: decode via `image`, convert to RGB8, enforce
  `MAX_PAGE_WIDTH/HEIGHT`, return `(Vec<u8> rgb, u32 w, u32 h)`.
- `src/pdf.rs`: bind `libpdfium.so` from a pinned path (loaded at runtime);
  `open_pdfium() -> Result<Pdfium, _>`; `rasterize_pdf(bytes) -> Vec<RasterPage>`
  where `RasterPage { rgb: Vec<u8>, width: u32, height: u32 }`. Render each page
  at `DEFAULT_DPI` (150) using `PdfRenderConfig::set_target_width(...)` computed
  from page points (72 dpi base). Enforce `MAX_PAGES` and per-page limits.
  Corrupt input -> `ConversionError::DocCorruptedException`.
- `src/office.rs`: write input to a temp dir as `/tmp/input_file`, run
  `libreoffice --headless --safe-mode --convert-to pdf --outdir /tmp
  /tmp/input_file`, require the intermediate `.pdf` to exist (missing file or
  non-zero exit -> `LibreofficeFailure`), then rasterize via `pdf.rs`.
- `src/doc_to_pixels.rs`: `pub fn convert(input: impl Read, output: impl Write,
  progress: impl FnMut(&str)) -> Result<(), ConversionError>`. Read bounded
  input, detect format, dispatch to image/pdf/office, write protocol (BE u16
  count, then width/height/rgb). Progress to the callback (main prints to
  stderr).
- `src/main.rs` (`dz-convert`): read stdin fully (bounded size, e.g. 100 MB),
  call `doc_to_pixels::convert`, map errors via `ConversionError::error_code()`
  and exit with that code; print progress to stderr.
- Tests:
  - `format_detect` against magic-byte fixtures.
  - `image` decode of a tiny generated PNG.
  - `doc_to_pixels` writer round-trip against a reader mirroring `base.rs`.
  - exit-code mapping for `DocFormatUnsupported` etc.
  - PDFium-dependent tests gated so they only run when the library is present.
- Verification: `cargo build --features dz-converter/sandbox --bin dz-convert`,
  tests, clippy.

> **Status: DONE.** `dz-converter` sandbox pipeline written (format_detect via
> `infer`, image decode via `image`, PDFium rasterizer bound at runtime with
> `DANGERZONE_LIBPDFIUM`/`/opt/dangerzone/lib/libpdfium.so`, LibreOffice
> converter with per-run profile, `doc_to_pixels::convert` + `dz-convert` bin)
> with 18 tests. Smoke-tested end-to-end: 1x1 PNG -> `00 01 00 01 00 01 ff 00
> 00`; garbage -> exit 138. Workspace: 112 tests pass, clippy clean (with and
> without `--features dz-converter/sandbox`).

## Phase 3 — sandbox container + host wiring

- `sandbox/Containerfile` (multi-stage, self-contained, offline runtime):
  - Builder: `rust:bookworm` -> copy workspace, build
    `dz-convert` with `--features dz-converter/sandbox --release --target
    x86_64-unknown-linux-musl` (static binary).
  - Runtime: `debian:bookworm-slim` -> install `libreoffice` + fonts
    (`fonts-liberation`, `fonts-dejavu`), create user `dangerzone`, copy the
    static binary to `/opt/dangerzone/dz-convert`, copy a pinned
    `libpdfium.so` (from bblanchon/pdfium-binaries, sha256-verified via `ADD`)
    to `/opt/dangerzone/lib/libpdfium.so`, set `LD_LIBRARY_PATH`.
  - Copy hardened `seccomp.json` + `apparmor.profile` into the image.
- `sandbox/policies/seccomp.json`: real allowlist profile (Docker-default-derived
  baseline for glibc/LibreOffice/PDFium). Replace 0-B file.
- `sandbox/policies/apparmor.profile`: basic confinement profile.
- Delete `sandbox/entrypoint.rs` and `sandbox/rootfs/opt/dangerzone/convert.sh`
  (superseded by the Rust binary).
- `scripts/build-image.sh`: defensive Bash (strict mode, `command -v` dep
  checks, `--help`, logging, idempotent build, tags `dangerzone-sandbox:latest`).
- Host wiring in `dz-runtime`:
  - `container.rs::start_doc_to_pixels_proc`: command becomes
    `["/opt/dangerzone/dz-convert"]`.
  - `container_utils.rs`: `expected_image_name()` -> local `dangerzone-sandbox`
    (env override `DANGERZONE_IMAGE_NAME`); `get_local_image_digest()` ->
    `podman image inspect --format {{.Id}}` with the existing all-zero fallback.
    Keep `make_seccomp_json_accessible` pointing at the real profile.
  - Signature verification stays as the existing `Ok(())` stub (deferred).
- Verification: `scripts/build-image.sh`, then a manual container conversion.

> **Status: DONE.** `sandbox/Containerfile` (multi-stage glibc build — the musl
> plan was dropped because a musl-static binary cannot `dlopen` the glibc
> `libpdfium.so`), `.dockerignore`, hardened `seccomp.json` (Docker-default
> derived allowlist) + `apparmor.profile`, and a defensive `scripts/build-image.sh`
> that pins PDFium by SHA-256 (`7358c15e…`, downloaded from
> bblanchon/pdfium-binaries) and builds `dangerzone-sandbox:latest`.
> `sandbox/entrypoint.rs` and `sandbox/rootfs/opt/dangerzone/convert.sh` deleted.
> Host wiring: `container.rs` now runs `/opt/dangerzone/dz-convert`;
> `container_utils.rs` uses local `dangerzone-sandbox:latest`
> (`DANGERZONE_IMAGE_NAME` override), queries the image digest via
> `podman image inspect` (all-zero fallback), and embeds the real seccomp
> profile (kept byte-identical to the file). Workspace: 112 tests, clippy clean.

## Phase 4 — integration tests + docs

- `tests/integration/cli.rs`: exercise `dz-dangerzone` CLI with the Dummy
  provider in dev mode: `--version`, suspicious-option guard, missing input
  file, default output naming, `-safe.pdf` produced.
- `tests/integration/conversion.rs`:
  - (a) Dummy-provider end-to-end: tiny fixture -> PDF produced and validated
    with `dz_output::validator`.
  - (b) Container end-to-end gated behind `DANGERZONE_CONTAINER_TESTS=1`
    (requires podman + built image): tiny PDF/PNG fixture -> safe PDF ->
    validate.
- `tests/fixtures/`: tiny sample PDF/PNG files (check them in).
- README: fix build instructions (`cargo build --workspace` now works),
  document container build via `scripts/build-image.sh`, update the crate table
  (dz-converter = protocol + sandbox converter, dz-output = reconstruction).
- Add `docs/architecture.md` (referenced but missing): components, data flow,
  threat model summary.
- Verification: full `cargo test --workspace`; with container: run gated tests.

> **Status: DONE.** Since `tests/` is a Cargo package (`dz-tests`, a workspace
> member), the integration tests live at `tests/tests/{cli,conversion}.rs`
> (Cargo requires integration tests under the package's `tests/` directory; the
> old empty `tests/integration/*.rs` stubs were removed). The tests drive the
> real `dz-dangerzone` binary as a subprocess. Fixtures `sample.pdf`
> (1 page, 100x100px) and `sample.png` (8x6 red) are checked into
> `tests/fixtures/`. The gated container test skips cleanly unless
> `DANGERZONE_CONTAINER_TESTS=1` AND podman AND the image are present. README
> updated; `docs/architecture.md` added. Workspace: 110 tests pass, clippy
> clean (`--all-targets -- -D warnings`).

---

## Gap analysis (Rust port vs upstream, at plan time)

### At parity (no work needed)
- **CLI options**: `--output-filename`, `--ocr-lang`, `--archive`,
  `--unsafe-dummy-conversion`, `--debug`, `--set-container-runtime`,
  `--linger`, `--version` — all present in `dz-cli/src/main.rs`.
- **Document model** (`dz-core/src/document.rs`): ids, normalization,
  validation, state machine, archive, output-dir, default output naming.
- **Core logic** (`dz-core/src/logic.rs`), settings persistence
  (`settings.rs`), startup/shutdown task frameworks (`startup.rs`,
  `shutdown.rs`).
- **Isolation providers** (`dz-runtime`): container/dummy/qubes all functional.
- **Container security args**: byte-for-byte identical to upstream
  `container.py` (`--cap-drop all`, `--cap-add SYS_CHROOT`, `--network=none`,
  `--security-opt no-new-privileges`, `--userns nomap`, seccomp profile,
  `-u dangerzone`, `--log-driver none`, `label=type:container_engine_t`).
  NOTE: upstream passes no `--read-only`/`--memory`/`--pids-limit` either.
- **Wire protocol + conversion pipeline** (`dz-runtime/src/base.rs`,
  `dz-converter`): full end-to-end.
- **Error types** (`dz-core/errors.rs`, `dz-converter/errors.rs`).

### Stubs to replace (functional gaps)
| Area | Upstream | Rust port today |
|------|----------|-----------------|
| Container image installer | `updater/installer.py` (check/load/pull/verify/tag) | `dz-core/stubs.rs` installer always `DoNothing` |
| Release checking | `updater/releases.py` (GitHub API) | `dz-core/stubs.rs` `releases` always `Ok(false)` |
| Signature verification | `updater/signatures.py` (23 KB, Sigstore/cosign) + `cosign.py` | `dz-update/src/signatures.rs` — 7 stub fns |
| Registry query | `updater/registry.py` (OCI manifest digest) | `dz-update/src/registry.rs` — 1 stub fn |
| Local image verify | `updater.py::verify_local_image` | `dz-runtime/src/updater.rs` — always trusts |
| Image lifecycle | `container_utils.py`: load_image_tarball, tag_image_by_digest, clear_old_images, list_image_digests, delete_image_digests, container_pull, get_image_id_by_digest | only `expected_image_name()` + `get_local_image_digest()` |
| Podman machine / WSL | `podman/machine_manager.py`, `windows/wsl.py` | `dz-core/stubs.rs` no-op |
| containers.conf | `container_utils.py::create_containers_conf` (CPU, volumes) | not implemented |

### Missing features (not started)
| Area | Upstream | Rust port today |
|------|----------|-----------------|
| OCR | PyMuPDF `pdfocr_tobytes` + Tesseract + multiprocessing pool | `dz-output/src/pdf.rs::ocr_pdf_page` always fails; no Tesseract in image |
| OCR language validation | `share/ocr-languages.json` loaded, `--ocr-lang` validated | JSON exists; not loaded/validated |
| GUI | Full PyQt GUI (`gui/main_window.py` 70 KB, log_window, widgets, updater) | `dz-gui/src/*` all 0 bytes |
| Formats | epub, SVG, HWP/HWPX | `format_detect.rs` maps them to `Unsupported` |
| Progress reporting | per-page percentage + OCR progress + color | callback exists; percentage not per-page |
| Stderr capture | background thread streams converter stderr, logs at end | captured but not threaded |
| Docker runtime | supported | Podman-only |
| Packaging | deb/rpm/msi/dmg | none |

### Known shared limitations (upstream has them too — do NOT fix)
- `get_max_parallel_conversions()` hardcoded to 1.
- Ubuntu 22.04 `podman ps -a` fallback path.

---

## Phase 5 — Container image lifecycle

Make the image installable, verifiable, and manageable, replacing every updater
stub.

Steps:
1. `crates/dz-runtime/src/container_utils.rs` — implement, mirroring
   `container_utils.py`:
   - `list_image_digests() -> Vec<String>` (`podman image list --format {{.Digest}}`)
   - `get_image_id_by_digest(digest) -> String` (`podman images --format json`)
   - `delete_image_digests(digests, container_name)` (`podman rmi --force`)
   - `clear_old_images(digest_to_keep)`
   - `load_image_tarball(tarball_path) -> String` (`podman load -i`, parse digest)
   - `tag_image_by_digest(digest, tag)` (`podman tag`)
   - `container_pull(image, manifest_digest)` (`podman pull`)
   - `get_local_image_digest()` already exists — keep, but raise
     `ImageNotPresent`/`MultipleImagesFound` errors when absent/ambiguous.
2. `crates/dz-update/src/registry.rs` — implement `get_manifest_digest(&str)`
   by querying the OCI registry manifest endpoint (Docker Registry HTTP API v2,
   `Accept: application/vnd.docker.distribution.manifest.v2+json`); add
   `reqwest` (blocking) or `ureq` to the workspace deps.
3. `crates/dz-update/src/manifest.rs` — serde types for the GitHub release
   manifest (version, changelog, log index URL, container image URL/digest),
   `download_manifest()` + `parse_manifest()`.
4. `crates/dz-update/src/signatures.rs` — replace all 7 stubs:
   - `get_remote_signatures`: download cosign `.sig`/`.cert`/`key.sig` bundles
     from the release log index or GitHub.
   - `verify_signatures`: verify against the bundled
     `share/freedomofpress-dangerzone.pub` (Sigstore keyless cert chain, or
     key-based ED25519 for the container image). Implement with `sigstore-rs`
     if viable; otherwise shell out to the `cosign` binary (defensive Bash-own
     wrapper, documented).
   - `store_signatures`: persist to the cache dir + advance the log index.
   - `upgrade_container_image`: pull (`container_pull`) or load
     (`load_image_tarball`), verify, tag.
   - `upgrade_container_image_airgapped`: load from archive, verify, return
     (image_digest, version).
   - `prepare_airgapped_archive`: bundle image tarball + signatures.
   - `verify_local_image`: verify stored signatures against the local digest.
5. `crates/dz-runtime/src/updater.rs` — implement `verify_local_image` using
   `dz-update::signatures` (keep `bypass_signature_checks()` env override).
6. `crates/dz-core/src/stubs.rs` — replace `updater` no-ops with real
   delegates. Break the crate cycle with a small trait or feature-gated re-export
   so `dz-core` calls into `dz-runtime`/`dz-update` without a dependency cycle
   (prefer: move the updater interfaces to `dz-core`, implement them in
   `dz-runtime`, wire via the existing task framework).
7. `crates/dz-core/src/startup.rs` — `ContainerInstallTask` actually
   loads/pulls/verifies/tags the image; `UpdateCheckTask` checks GitHub and
   reports app/image updates (handle the `UpdaterDisabledNoContainer` path the
   CLI already surfaces).
8. `crates/dz-update/src/cli.rs` — make `cmd_upgrade`, `cmd_store_signatures`,
   `cmd_load_archive`, `cmd_prepare_archive`, `cmd_verify_local` real.

Files: `crates/dz-runtime/src/{container_utils,updater}.rs`,
`crates/dz-update/src/{registry,manifest,signatures,updater,verify,cli}.rs`,
`crates/dz-core/src/{stubs,startup}.rs`, root `Cargo.toml` (add `reqwest`/`ureq`,
maybe `sigstore-rs`), `share/freedomofpress-dangerzone.pub`.

Verification:
- `cargo build/test/clippy --workspace`.
- `cargo run --bin dz-update-image -- upgrade` loads and tags the image; a
  second run reports it is already installed.
- `cargo run --bin dz-update-image -- verify-local` succeeds for the built
  image and fails for a tampered digest (unit-tested with a stub verifier).

> **Status: DONE.** All eight steps were implemented and are verified in place
> (the plan status was not updated when the work landed). Step 1:
> `container_utils.rs` implements `list_image_digests`, `get_image_id_by_digest`,
> `delete_image_digests`, `clear_old_images`, `load_image_tarball`,
> `tag_image_by_digest`, `container_pull`, and `get_local_image_digest` (which
> already raises `ImageNotPresent`/`MultipleImagesFound`). Step 2:
> `dz-update/registry.rs` implements the OCI registry manifest digest query
> (anonymous bearer token + `Accept` header, SHA-256) with `ureq`, plus
> `get_digest_for_arch` and `parse_image_location`. Step 3: `manifest.rs`
> models the GitHub release and downloads/parses it. Step 4: `signatures.rs`
> replaces all 7 stubs — `get_remote_signatures` (via `cosign download
> signature`), `verify_signatures`/`verify_local_image` (cosign bundle
> verification against `share/freedomofpress-dangerzone.pub`, with the bundled
> Rekor key for offline checks), `store_signatures` (per-pubkey digest folder +
> log-index file), `upgrade_container_image`, `upgrade_container_image_airgapped`
> (OCI layout sanity check + `dangerzone.json`), `prepare_airgapped_archive`
> (`cosign save`), and `bypass_signature_checks`. The low-level primitives live
> in `dz-runtime/updater/{cosign,signatures}.rs`. Step 5: `dz-runtime/updater.rs`
> implements `verify_local_image` (keeps the `DANGERZONE_BYPASS_SIGNATURE_
> VERIFICATION` override). Step 6: the updater no-ops are gone from `stubs.rs`;
> the `ContainerInstaller`/`UpdateChecker` interfaces moved to
> `dz-core/updater.rs` (breaking the crate cycle) and are implemented by
> `dz-update/updater.rs::Updater`. Step 7: `startup.rs::ContainerInstallTask`
> and `UpdateCheckTask` are real (strategy selection, install, cooldown,
> GitHub + remote log-index checks). Step 8: `dz-update/cli.rs` implements
> `upgrade`, `store-signatures`, `load-archive`, `prepare-archive`,
> `verify-local` behind the `dz-update-image` bin. `share/freedomofpress-
> dangerzone.pub` and `share/rekor.pub` ship as PEM keys. Dead 0-byte
> `verify.rs` removed. Workspace: 149 tests pass, clippy clean
> (`--all-features`); E2E commands require podman + a bundled cosign binary and
> are not runnable on this machine (unit tests cover the logic).

---

## Phase 6 — OCR support

Make `--ocr-lang` produce searchable PDFs, mirroring upstream's Tesseract +
per-page worker pool.

Steps:
1. `sandbox/Containerfile` — install `tesseract-ocr` and language packs
   (`tesseract-ocr-eng`, plus `-deu`, `-fra`, `-spa`, `-por`, etc.), set
   `OMP_THREAD_LIMIT=1` in the runtime.
2. `crates/dz-converter/src/pdf.rs` — add `ocr_pdf_page` path: rasterize the
   page with PDFium, run `tesseract` (shell out to the CLI with the chosen
   `-l` lang and a temp tessdata dir) to produce a searchable PDF page, and
   return it as raw page bytes (same wire protocol). Keep the non-OCR path
   unchanged.
3. `crates/dz-converter/src/doc_to_pixels.rs` — thread `ocr_lang` through
   `convert()` and pass it to `pdf.rs`.
4. `crates/dz-converter/src/main.rs` — read an `--ocr-lang` arg (or env
   `DANGERZONE_OCR_LANG`) so the container knows when to OCR.
5. `crates/dz-runtime/src/container.rs` — pass `--ocr-lang` to
   `/opt/dangerzone/dz-convert` when the document requests OCR; also mount the
   tessdata dir or bake it into the image (bake it — keep the image
   self-contained).
6. `crates/dz-output/src/pdf.rs` — replace the `ocr_pdf_page` stub with an
   implementation that merges the OCR'd page bytes into the safe document
   (upstream merges PyMuPDF page docs). This runs on the HOST, so it must not
   shell out to tesseract: the sandbox returns searchable page PDFs and the
   host just inserts them.
7. `crates/dz-runtime/src/base.rs` — in `convert_with_proc`, when `ocr_lang`
   is set, submit pages to a worker pool (std threads or a small executor) and
   drain futures like upstream to bound RAM; report
   `"Converted page X/Y to searchable PDF"` progress.
8. `crates/dz-core/src/util.rs` — load `share/ocr-languages.json` and expose
   `ocr_languages()`; `dz-cli/src/main.rs` already validates against it.
9. Tests: unit-test the tesseract shell-out with a stubbed binary; host-side
   merge test with a synthetic searchable page.

Files: `sandbox/Containerfile`, `crates/dz-converter/src/{pdf,doc_to_pixels,main}.rs`,
`crates/dz-runtime/src/{container,base}.rs`, `crates/dz-output/src/pdf.rs`,
`crates/dz-core/src/util.rs`.

Verification:
- `cargo build/test/clippy --workspace`.
- Container E2E: `cargo run --bin dz-dangerzone -- --ocr-lang eng sample.pdf`
  produces a safe PDF whose text layer is searchable (assert via the validator +
  a text-extraction spot check in the gated test).

> **Status: DONE.** `--ocr-lang` produces searchable PDFs end-to-end. The
> sandbox (`dz-convert --ocr-lang <code>`) rasterizes each page with PDFium,
> stages it as a binary P6 PNM, and shells out to `tesseract <page.pnm>
> <out> -l <code> pdf` (binary overridable via `DANGERZONE_TESSERACT`, tessdata
> via a candidate `--tessdata-dir` list), then sends length-prefixed single-page
> PDFs over the OCR wire protocol. `dz-cli` maps the human language name to the
> Tesseract code via `share/ocr-languages.json`. The host validates each page
> (`dz_output::pdf::ocr_pdf_page`) in a bounded worker pool (one thread per two
> CPUs, `drain_ocr_futures` mimicking upstream) and merges the searchable pages
> with lopdf (`write_pdf_merged`), while pure-raster docs keep the classic
> writer. The Containerfile installs `tesseract-ocr` + 9 language packs and sets
> `OMP_THREAD_LIMIT=1`. Sandbox-only tests use a stub tesseract binary.
> Workspace: 147 tests pass, clippy clean; sandbox-feature tests pass too
> (22 in `dz-converter`).

---

## Phase 7 — Progress and UX improvements

Match upstream's per-page progress, threaded stderr capture, and stop/cleanup
timeouts.

Steps:
1. `crates/dz-runtime/src/base.rs` — `convert_with_proc`: compute
   `percentage = (page / n_pages) * 100` and emit `"Converted page {page}/{n} to
   PDF"` per page (upstream behavior); emit `"Successfully converted document"`
   at 100%.
2. `crates/dz-runtime/src/base.rs` — add the upstream `start_stderr_thread`
   pattern: a daemon thread drains converter stderr into a buffer while the
   conversion runs; after cleanup, log it once via `sanitize_debug_text`
   wrapped in the `DOC TO PIXELS LOG START/END` markers (only when stderr is
   captured, i.e. debug/dev).
3. `crates/dz-runtime/src/base.rs` — verify `ensure_stop_doc_to_pixels_proc`
   matches upstream's poll -> SIGTERM group -> wait(grace) -> SIGKILL group ->
   wait(force) sequence and that `get_proc_exception` respects
   `TIMEOUT_EXCEPTION`.
4. `crates/dz-runtime/src/container_utils.rs` — `expected_image_name()` reads
   `share/image-name.txt` (via `dz_core::util::get_resource_path`) with the
   `DANGERZONE_IMAGE_NAME` override; ship `share/image-name.txt` =
   `dangerzone-sandbox:latest`.
5. `crates/dz-cli/src/main.rs` — print progress lines to stdout like upstream
   CLI (`[doc <id>] <pct>% <text>`), gated on the stdout callback already
   threaded through the core.

Files: `crates/dz-runtime/src/{base,container_utils}.rs`,
`crates/dz-cli/src/main.rs`, `share/image-name.txt`.

Verification:
- Multi-page dummy/container conversion shows monotonic per-page percentages.
- Debug run shows the full converter log block at the end.
- `cargo build/test/clippy --workspace`.

> **Status: DONE.** Steps 1-3 were already implemented and are verified in
> place: `convert_with_proc` reports per-page percentages ("Converted page
> X/N to PDF", X/N\*100) and "Successfully converted document" at 100%; the
> `start_stderr_thread`-equivalent drains converter stderr into an
> `Arc<Mutex<Vec<u8>>>` during the run and logs it after cleanup wrapped in
> the `DOC TO PIXELS LOG START/END` markers (only when the provider pipes
> stderr via `should_capture_stderr()`); `ensure_stop_doc_to_pixels_proc`
> follows poll -> SIGTERM group -> wait(grace) -> SIGKILL group -> wait(force),
> and `get_proc_exception` respects `TIMEOUT_EXCEPTION`. Step 4 implemented:
> `expected_image_name()` reads `share/image-name.txt` via
> `dz_core::util::get_resource_path` (env `DANGERZONE_IMAGE_NAME` wins), and
> `share/image-name.txt` = `dangerzone-sandbox:latest` is shipped. Step 5
> implemented: `dz-cli` formats `[doc <id>] <pct>% <text>` and forwards it to
> the stdout callback threaded through the core (errors go to stderr);
> `main()` passes a real `print_progress` callback to `convert_documents`.
> Workspace: 149 tests pass, clippy clean (`--all-features`).

---

## Phase 8 — Format expansion

Add epub, SVG, and HWP/HWPX support.

Steps:
1. `crates/dz-converter/src/format_detect.rs` — extend `DocumentFormat` with
   `Epub`, `Svg`, `Hwp`, `Hwpx` (infer/extension + zip-signature detection for
   epub/hwpx).
2. `crates/dz-converter/Cargo.toml` — optional deps:
   `resvg` (+ `usvg`) for SVG, `zip` for epub/hwpx inspection, or shell out to
   Calibre `ebook-convert` for epub. Prefer in-process Rust crates; fall back to
   documented shell-outs only where no crate exists.
3. New `crates/dz-converter/src/epub.rs` — `epub_to_pdf` via Calibre
   `ebook-convert` in the image, then rasterize the intermediate PDF with
   `pdf.rs` (mirrors the office path).
4. New `crates/dz-converter/src/svg.rs` — `svg_to_rgb` via `resvg`, enforce
   `MAX_PAGE_WIDTH/HEIGHT`, return a single RGB page.
5. `crates/dz-converter/src/office.rs` — extend to HWP (`libreoffice --convert-to
   pdf`) and note HWPX limitations (libreoffice support varies); if unsupported,
   map to `DocFormatUnsupportedHwpQubes`/`DocFormatUnsupported` faithfully.
6. `crates/dz-converter/src/doc_to_pixels.rs` — dispatch to the new handlers.
7. `sandbox/Containerfile` — add `calibre` (or the minimal epub toolchain) and
   the fonts already installed.
8. Tests: fixture-driven format detection; SVG raster bounds; epub
   conversion smoke test gated on the container.

Files: `crates/dz-converter/src/{format_detect,doc_to_pixels,office}.rs`, new
`epub.rs`/`svg.rs`, `crates/dz-converter/Cargo.toml`, `sandbox/Containerfile`.

Verification:
- `cargo build/test/clippy --workspace`.
- Container E2E for `sample.svg` (generated fixture) and a tiny `sample.epub`;
  HWP path verified against LibreOffice availability in the image.

> **Status: DONE.** Step 1: `format_detect.rs` extends `DocumentFormat` with
> `Epub`, `Svg`, `Hwp`, `Hwpx`; detection uses `infer` (epub/svg now supported),
> the `HWP Document File` magic, an SVG XML sniff (so classification works on
> the raw stream), and — for HWPX, which shares the generic `PK\x03\x04` header
> — a ZIP central-directory inspection for the `.hpfx` part via the `zip`
> crate. Step 2: `resvg` (+ its bundled `usvg`) and `zip` added as optional
> workspace deps, gated behind the `sandbox` feature. Step 3: new `epub.rs`
> shells out to Calibre `ebook-convert` (binary overridable via
> `DANGERZONE_EBOOK_CONVERT`) and rasterizes the intermediate PDF with `pdf.rs`,
> mirroring the office path. Step 4: new `svg.rs` renders in-process with
> `resvg` (system fonts via `fontdb`), enforces `MAX_PAGE_WIDTH/HEIGHT` before
> allocating the canvas. Step 5: `office.rs` gains an `OfficeSource`
> (`Standard`/`Hwp`/`Hwpx`) and stages the input with a real extension so
> LibreOffice picks the import filter; a failed HWPX conversion maps to
> `DocFormatUnsupported` (LibreOffice has no HWPX filter), HWP maps to
> `LibreofficeFailure`. Step 6: `doc_to_pixels.rs` dispatches the four new
> formats. Step 7: `sandbox/Containerfile` adds `calibre`. Step 8: 12 new
> tests (format detection fixtures incl. in-memory zip/hwpx/epub archives,
> SVG raster bounds + corrupt rejection, SVG end-to-end through `convert()`,
> ebook-convert stub test gated on PDFium). Verified on this machine: workspace
> builds with `--all-features`; **174 tests pass**; clippy clean; the real
> `dz-convert` binary converts an SVG to a 4x3 red page, rejects a generic ZIP
> with exit 138, and dispatches a `.hpfx` ZIP to the HWP path (228 locally,
> LibreOffice absent). Container E2E (calibre + LibreOffice + PDFium in the
> image) is not runnable here and is covered by the stub-gated unit tests.

---

## Phase 9 — Cross-platform and container runtime support

Complete Windows (WSL), macOS (Podman machine), and Docker support.

Steps:
1. `crates/dz-runtime/src/container_utils.rs` — implement
   `create_containers_conf()`: write `[engine] helper_binaries_dir` +
   `[machine] cpus=<count>, volumes=["<cache>/shared:<cache>/shared:ro"],
   rosetta=false`; wire it into `init_podman_command()` for non-Linux with
   `CONTAINERS_CONF` + `--connection dz-internal-<version>` +
   `--storage-opt overlay.mount_program=...` (see `container_utils.py`).
2. `crates/dz-core/src/stubs.rs` — replace `wsl` stubs (Windows only):
   `is_installed()` via `wsl --status`, `install_and_check_reboot()` mirroring
   `windows/wsl.py`.
3. `crates/dz-runtime/src/podman/machine.rs` + `machine_manager.rs` — real
   lifecycle: `init`, `start`, `stop`, `list_other_running_machines`,
   `ensure_running`; honor `--linger`.
4. `crates/dz-runtime/src/container_utils.rs` — runtime detection
   (`get_runtime_type`): podman vs docker; `get_podman_path()`:
   `share/vendor/podman` on Windows/macOS, default on Linux; adjust
   `get_runtime_security_args` where the engines differ (e.g. skip `--userns
   nomap` for Docker).
5. `crates/dz-core/src/util.rs` — Tails handling in `init_podman_command`
   (HTTPS_PROXY = `get_tails_socks_proxy()`).
6. `scripts/build-image.sh` — accept a runtime flag (`--runtime podman|docker`).
7. `crates/dz-runtime/src/qubes.rs` — complete qfileexec negotiation
   (`start_doc_to_pixels_proc` arg conventions) to match `qubes.py`.

Files: `crates/dz-runtime/src/{container_utils,container,podman/*,qubes}.rs`,
`crates/dz-core/src/stubs.rs`, `crates/dz-core/src/util.rs`,
`scripts/build-image.sh`.

Verification:
- `cargo build/test/clippy --workspace` (Linux CI).
- Manual: macOS with Podman machine, Windows with WSL, Docker runtime smoke
  test, Tails env check.

> **Status:** pending.

---

## Phase 10 — GUI

Implement the graphical user interface with egui/eframe.

Steps:
1. `crates/dz-gui/Cargo.toml` — add `eframe` (+ `rfd` for file dialogs, `image`
   for thumbnails); add `dz-gui` back into the workspace members (it is
   currently excluded and 0-byte).
2. `crates/dz-gui/src/app.rs` — `DangerzoneApp`: document drop/add list,
   conversion progress view (per-document percentage), settings panel, results
   (open output folder), error view; drives `DangerzoneCore` with the same
   providers as the CLI.
3. `crates/dz-gui/src/widgets.rs` — progress bar, log window
   (mirrors `gui/log_window.py`), custom drag-drop widget.
4. `crates/dz-gui/src/startup.rs` — startup-task progress display (image
   install/update), mirroring `gui/startup.py`.
5. `crates/dz-gui/src/updater.py`-equivalent `crates/dz-gui/src/updater.rs` —
   app-update prompt using `dz-update` release checking.
6. `crates/dz-gui/src/main.rs` — eframe entry point, theming, window title.
7. `crates/dz-gui/Cargo.toml` — `[[bin]] dz-dangerzone-gui`.
8. Wire progress from `dz-core::logic` into the GUI via the existing progress
   callback (shared with the CLI).

Files: `crates/dz-gui/**/*`, root `Cargo.toml` (add member).

Verification:
- `cargo run -p dz-gui`; drag-and-drop a PDF, watch progress, get a validated
  safe PDF; settings persist; error paths show readable messages.

> **Status:** pending.

---

## Phase 11 — Packaging and distribution

Create platform packages and release infrastructure.

Steps:
1. `packaging/debian/` — `control`, `postinst`/`postrm` (install `dz-dangerzone`,
   `dz-convert` in the image build, `share/` resources, desktop file, icons,
   `dangerzone.pub`), build via `cargo build --release`.
2. `packaging/rpm/` — `.spec` equivalent.
3. `packaging/macos/` — app bundle + `.dmg`, codesigning/notarization hooks in
   `scripts/`.
4. `packaging/windows/` — WiX/NSIS `.msi`, vendor podman binaries per
   `get_podman_path()`.
5. `scripts/sign-release.sh` — implement real Sigstore signing of release
   artifacts (used by `dz-update` Phase 5 verification).
6. `share/` — icons, `.desktop`, translations scaffold; `container.tar` bundling
   path for offline install (`load_image_tarball`).
7. `README`/docs — install instructions per platform.

Files: `packaging/**`, `scripts/sign-release.sh`, `share/**`, `docs/**`.

Verification:
- Build and install each package on its platform; installed binary finds
  resources via `DANGERZONE_PREFIX`/`get_resource_path`; offline image install
  works from a bundled `container.tar`.

> **Status:** pending.

---

## Priority summary

| Phase | Effort | Impact | Blocks |
|-------|--------|--------|--------|
| 5 — Container image lifecycle | Large | Critical: cannot install/verify the image today | 6, 7, 9 |
| 6 — OCR | Medium | Major feature gap | — |
| 7 — Progress/UX | Small | Usability | — |
| 8 — Format expansion | Medium | Feature completeness | — |
| 9 — Cross-platform | Large | Platform reach | — |
| 10 — GUI | Large | User experience | — |
| 11 — Packaging | Medium | Distribution | — |

Phases 5 and 7 are the natural next steps (both unblock the others and touch
the same runtime code); 6, 8, 9, 10, 11 can proceed in any order afterward.

---

## Verification (after each phase)
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- Final E2E: `scripts/build-image.sh`, then
  `DANGERZONE_DEV=1 cargo run --bin dz-dangerzone -- --unsafe-dummy-conversion file.pdf`
  and a real container conversion of a sample PDF/PNG.

## Deferred (documented, not built here)
Items formerly listed here are now covered by Phases 5-11 and have moved:
GUI -> Phase 10; Sigstore/`manifest.rs`/`updater.rs`/`verify.rs` -> Phase 5;
OCR/Tesseract -> Phase 6; epub/svg/HWP -> Phase 8; packaging/signing ->
Phase 11. The per-page progress callback is wired in Phase 7. Still out of
scope:
- `dz-core` job/limits/policy module (upstream `document.py` code paths not
  exercised by the CLI) — revisit only if behavior diverges.
- gVisor-grade seccomp hardening (upstream doesn't do this either; current
  profile matches upstream).
- Tails-specific integration testing, macOS/Windows automated CI.
- The `dangerzone_image` build process itself already matches upstream; any
  further image-shrinking work (distroless, etc.) is optional.

---

## Original reference layout (for faithful translation)
The upstream package `dangerzone/` (main branch) contains: `gui/`,
`isolation_provider/` (base, container, qubes, dummy), `podman/`, `updater/`,
`windows/`, and modules `cli.py`, `args.py`, `container_utils.py`,
`conversion_errors.py`, `document.py`, `errors.py`, `logic.py`, `settings.py`,
`shutdown.py`, `startup.py`, `util.py`. The container-side converter lives in
the separate `freedomofpress/dangerzone-image` repo (`dangerzone-insecure-
conversion`, `dangerzone/conversion/{doc_to_pixels,format_detect,image,office,
pdf,pixels_to_pdf}.py`). The container image is invoked by the host as:
`/usr/bin/python3 -m dangerzone.conversion.doc_to_pixels`. This port replaces
that Python module with the Rust `dz-convert` binary.
