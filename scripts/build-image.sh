#!/bin/bash
set -Eeuo pipefail

# build-image.sh - build the Dangerzone conversion sandbox container image.
#
# Stages the pinned PDFium library (downloaded from bblanchon/pdfium-binaries,
# verified by SHA-256), then builds the `dangerzone-sandbox` image from
# `sandbox/Containerfile`. Safe to re-run: a fully cached build is a no-op.

# --- Configuration -----------------------------------------------------------

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
CONTAINERFILE="$REPO_ROOT/sandbox/Containerfile"

IMAGE_NAME="${DANGERZONE_IMAGE_NAME:-dangerzone-sandbox:latest}"
CACHE_DIR="${DANGERZONE_CACHE_DIR:-$REPO_ROOT/.cache}"
RUNTIME="${DANGERZONE_CONTAINER_RUNTIME:-podman}"

PDFIUM_URL="${DANGERZONE_PDFIUM_URL:-https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-linux-x64.tgz}"
# Pin the SHA-256 of the tarball served by PDFIUM_URL. Bump it together with
# the URL whenever the pinned PDFium build changes.
PDFIUM_SHA256="${DANGERZONE_PDFIUM_SHA256:-7358c15e26a746cd67854887ea11b3b807c436056788eee9294fb972b8f8e0be}"

PDFIUM_TARBALL="$CACHE_DIR/pdfium-linux-x64.tgz"
PDFIUM_LIB="$CACHE_DIR/pdfium/lib/libpdfium.so"

BUILD_ARGS=()
EXTRA_BUILD_ARGS=()

log_info() { printf '[build-image] INFO: %s\n' "$*" >&2; }
log_error() { printf '[build-image] ERROR: %s\n' "$*" >&2; }

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Builds the Dangerzone conversion sandbox container image.

Options:
    -n, --no-pdfium   Do not download/verify PDFium; fail if it is not already staged.
    -a, --arg ARG     Extra argument passed to the container runtime's build command (repeatable).
    --runtime RUNTIME Use the given container runtime for building the image (`podman` or `docker`).
    -h, --help        Show this help message.

Environment:
    DANGERZONE_CONTAINER_RUNTIME  Runtime to use when building the image (`podman` or `docker`, default: podman).
    DANGERZONE_IMAGE_NAME         Image tag to build (default: dangerzone-sandbox:latest).
    DANGERZONE_PDFIUM_URL         URL of the PDFium tarball.
    DANGERZONE_PDFIUM_SHA256      Expected SHA-256 of that tarball.
    DANGERZONE_CACHE_DIR          Directory for staged downloads.
EOF
    exit "${1:-0}"
}

# --- Argument parsing ---------------------------------------------------------

while [[ $# -gt 0 ]]; do
    case "$1" in
        -n | --no-pdfium)
            NO_PDFIUM=true
            shift
            ;;
        -a | --arg)
            EXTRA_BUILD_ARGS+=("$2")
            shift 2
            ;;
        --runtime)
            if [[ $# -lt 2 ]]; then
                log_error "--runtime requires an argument"
                usage 1
            fi
            RUNTIME="$2"
            shift 2
            ;;
        -h | --help)
            usage 0
            ;;
        --)
            shift
            break
            ;;
        *)
            log_error "Unknown option: $1"
            usage 1
            ;;
    esac
done

# --- Dependency checks --------------------------------------------------------

require_cmd() {
    local -r cmd="$1"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        log_error "required command not found: $cmd"
        return 1
    fi
}

check_dependencies() {
    local -a required=()

    case "$RUNTIME" in
        podman)
            required+=(podman)
            ;;
        docker)
            required+=(docker)
            ;;
        *)
            log_error "unsupported runtime: $RUNTIME"
            return 1
            ;;
    esac

    if [[ "${NO_PDFIUM:-false}" != "true" ]]; then
        required+=(curl sha256sum tar)
    fi

    for cmd in "${required[@]}"; do
        require_cmd "$cmd" || return 1
    done
}

[[ -f "$CONTAINERFILE" ]] || { log_error "Containerfile not found: $CONTAINERFILE"; exit 1; }
check_dependencies

# --- Stage and verify PDFium ---------------------------------------------------

stage_pdfium() {
    mkdir -p "$CACHE_DIR"
    if [[ ! -f "$PDFIUM_TARBALL" ]]; then
        log_info "downloading PDFium: $PDFIUM_URL"
        curl -fL --retry 3 -o "$PDFIUM_TARBALL" "$PDFIUM_URL" || {
            log_error "failed to download PDFium from $PDFIUM_URL"
            return 1
        }
    else
        log_info "using cached tarball: $PDFIUM_TARBALL"
    fi

    local actual
    actual="$(sha256sum "$PDFIUM_TARBALL" | awk '{print $1}')"
    if [[ "$actual" != "$PDFIUM_SHA256" ]]; then
        log_error "PDFium SHA-256 mismatch: expected $PDFIUM_SHA256, got $actual"
        log_error "update DANGERZONE_PDFIUM_SHA256 if you intentionally bumped the pinned build"
        return 1
    fi
    log_info "PDFium SHA-256 verified: $actual"

    if [[ ! -f "$PDFIUM_LIB" ]]; then
        log_info "extracting libpdfium.so"
        rm -rf "$CACHE_DIR/pdfium"
        mkdir -p "$CACHE_DIR/pdfium"
        tar -xzf "$PDFIUM_TARBALL" -C "$CACHE_DIR/pdfium"
    fi
    [[ -f "$PDFIUM_LIB" ]] || { log_error "libpdfium.so not found in the tarball"; return 1; }
}

if [[ "${NO_PDFIUM:-false}" == "true" ]]; then
    [[ -f "$PDFIUM_LIB" ]] || { log_error "--no-pdfium given but $PDFIUM_LIB is not staged"; exit 1; }
else
    stage_pdfium
fi

BUILD_ARGS+=(--build-arg "PDFIUM_SO=$PDFIUM_LIB")

# --- Build the image ------------------------------------------------------------

log_info "building image: $IMAGE_NAME using runtime: $RUNTIME"
log_info "build context: $REPO_ROOT"

if [[ "$RUNTIME" == "docker" ]]; then
    docker build \
        "${EXTRA_BUILD_ARGS[@]}" \
        -f "$CONTAINERFILE" \
        "${BUILD_ARGS[@]}" \
        -t "$IMAGE_NAME" \
        "$REPO_ROOT" || {
        log_error "image build failed"
        exit 1
    }
else
    podman build \
        "${EXTRA_BUILD_ARGS[@]}" \
        -f "$CONTAINERFILE" \
        "${BUILD_ARGS[@]}" \
        -t "$IMAGE_NAME" \
        "$REPO_ROOT" || {
        log_error "image build failed"
        exit 1
    }
fi

log_info "image built successfully: $IMAGE_NAME"
