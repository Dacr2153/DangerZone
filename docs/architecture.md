# Dangerzone-RS Architecture

## Overview

Dangerzone-RS is a Rust port of
[Dangerzone](https://github.com/freedomofpress/dangerzone): it converts
untrusted documents into **safe PDFs** by rasterizing them inside a disposable
sandbox and rebuilding a brand-new PDF from the pixel buffers. The hostile
input never reaches a parser running on the host.

## Components

```
+---------------------+          +-----------------------------------------------+
|   dz-cli            |          |   dz-converter  (in the container, as        |
|  (dz-dangerzone)    |          |   the `dz-convert` binary)                   |
+----------+----------+          +-----------------------------------------------+
           |                       |            |            |                  |
           v                       |            v            v                  |
+---------------------+   doc_to_pixels | format_detect | pdf (PDFium)         |
|   dz-runtime        |      protocol   | image (image) | office (LibreOffice)  |
|  (container/qubes/  +----------------> |              |                      |
|   dummy providers)  |   stdin bytes   +--------------------------------------+
+----------+----------+                 | stdout: BE u16 page count, then per  |
           |                            | page u16 w, u16 h, w*h*3 RGB;        |
           v                            | stderr: progress; exit codes: errors  |
+---------------------+                 +--------------------------------------+
|   dz-output         |
|  (PDF reconstruction |
|   + validation)     |
+----------+----------+
           |
           v
    safe PDF (validated)
```

- **`dz-core`** — document model and validation (`Document`), conversion
  orchestration (`DangerzoneCore`), startup/shutdown task framework, settings,
  and shared error types. No sandbox knowledge.
- **`dz-runtime`** — isolation providers implementing
  `IsolationProvider::convert`. The **container** provider launches the sandbox
  image with podman (seccomp/AppArmor profile, read-only rootfs, resource
  limits, disposable container); the **dummy** provider runs a two-page
  solid-color converter for tests; **Qubes** is a stub.
- **`dz-converter`** — the sandbox-side pipeline. `format_detect` identifies the
  document type, `pdf` rasterizes via PDFium (a pinned, SHA-256-verified
  `libpdfium.so`), `image` decodes raster images with the `image` crate,
  `office` drives `libreoffice --headless --safe-mode` to convert office files
  to PDF first, and `doc_to_pixels` streams the pixel protocol on stdout.
  All size/count limits are enforced inside the sandbox.
- **`dz-output`** — takes the raw pixel buffers on the host and reconstructs a
  PDF from scratch with `lopdf`: no timestamps (deterministic output), a fixed
  `/Info` dictionary authored by the sanitizer, Flate-compressed image XObjects.
  `validator` re-parses the result and rejects any active content that must not
  survive sanitization.
- **`dz-update`** — container image installation and updater logic. The real
  Sigstore/Cosign verification is deferred; the current build stubs the update
  checks.

## Data Flow (container conversion)

1. The CLI validates the input filename and produces an absolute path.
2. `dz-runtime` spawns `podman run` with the `dangerzone-sandbox` image,
   mounting the input read-only and giving the container no network.
3. The container runs `/opt/dangerzone/dz-convert`. It reads the document bytes
   from stdin, rasterizes every page (enforcing `MAX_PAGES`, `MAX_PAGE_*` and
   `MAX_INPUT_BYTES`), and writes the pixel protocol to stdout.
4. The host reads the protocol, streams progress messages, and feeds each page
   buffer to `dz-output`, which assembles the safe PDF at the requested output
   path.
5. `dz-output::validator` parses the serialized PDF and fails closed if any
   forbidden feature (JavaScript, embedded files, launch actions, forms, ...)
   is present.

## Wire Protocol

The host and the sandbox exchange only pixels, not structured document data:

```
stdin : the raw bytes of the untrusted document
stdout: u16 BE page count,
        then per page: u16 BE width, u16 BE height, width*height*3 bytes RGB
stderr: human-readable progress messages
exit  : the conversion error code (see dz-converter::errors)
```

`DEFAULT_DPI=150`, `MAX_PAGES=10_000`, `MAX_PAGE_WIDTH=HEIGHT=10_000`,
`MAX_INPUT_BYTES=100 MiB`.

## Threat Model Summary

The trust boundary is the host. Anything that parses the untrusted file runs
inside the sandbox container:

- **Rendering parsers** (PDFium, the `image` crate, LibreOffice) never run on
  the host.
- The container is **ephemeral**, has **no network**, a **read-only root
  filesystem**, runs as a **non-root user**, and is confined by **seccomp** and
  **AppArmor** profiles plus resource limits.
- The host only consumes **raw pixel buffers** and a small structured header
  (page count, dimensions); every field is bounds-checked against the same
  limits enforced in the sandbox.
- The reconstructed PDF is **authored by the sanitizer**, carries **no
  inherited metadata** and **no timestamps**, and is **re-validated** before it
  is handed back to the user. A compromised container therefore cannot inject
  active content into the output.

## Key Security Properties

- The original file never touches host parsers.
- Output determinism: the same input always produces byte-identical safe PDFs.
- Defense in depth: limits in the sandbox, bounds checks in the host protocol
  reader, and independent validation of the final PDF.
