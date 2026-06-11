#!/usr/bin/env bash
set -euo pipefail

# Mimir install script
# Builds the release binary and runs the interactive initialiser.
#
# Usage: install.sh [options]
#   -p, --prefix DIR   Install directory (default: ~/.local/bin)
#   -f, --force        Overwrite existing binary
#   -h, --help         Show this help message

SCRIPT_NAME="${0##*/}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
INSTALL_PREFIX="${INSTALL_PREFIX:-$HOME/.local/bin}"
FORCE=false

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf '\033[1;34m[INFO]\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m[WARN]\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31m[ERROR]\033[0m %s\n' "$*" >&2; }
die()   { error "$*"; exit 1; }

usage() {
    cat <<USAGE
Mimir install script

Usage:
  $SCRIPT_NAME [options]

Options:
  -p, --prefix DIR   Install directory (default: \$HOME/.local/bin)
  -f, --force        Overwrite existing binary without prompting
  -h, --help         Show this help message

Environment:
  INSTALL_PREFIX     Same as --prefix

The script will:
  1. Check that Rust / Cargo are available
  2. Build the release binary (cargo build --release)
  3. Copy the binary to the install prefix
  4. Run \`mimir init\` interactively so you can set identity and optional systemd
USAGE
}

# ---------------------------------------------------------------------------
# Parse args
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        -p|--prefix)
            INSTALL_PREFIX="$2"
            shift 2
            ;;
        -f|--force)
            FORCE=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            error "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Prerequisites
# ---------------------------------------------------------------------------
require_cmd() {
    local cmd="$1"
    local reason="${2:-$cmd}"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        die "$reason is required but not found in PATH."
    fi
}

require_cmd cargo "Rust / Cargo"
require_cmd rustc "Rust / rustc"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
build_release() {
    info "Building Mimir release binary …"
    cd "$PROJECT_ROOT"
    cargo build --release
    info "Build complete."
}

# ---------------------------------------------------------------------------
# Install binary
# ---------------------------------------------------------------------------
install_binary() {
    local src="$PROJECT_ROOT/target/release/mimir"
    local dst="$INSTALL_PREFIX/mimir"

    if [[ ! -f "$src" ]]; then
        die "Binary not found at $src — did the build fail?"
    fi

    if [[ ! -d "$INSTALL_PREFIX" ]]; then
        info "Creating install directory: $INSTALL_PREFIX"
        mkdir -p "$INSTALL_PREFIX" || die "Failed to create $INSTALL_PREFIX"
    fi

    if [[ -f "$dst" && "$FORCE" != true ]]; then
        local reply
        printf 'Binary already exists at %s. Overwrite? [y/N]: ' "$dst"
        read -r reply
        case "$reply" in
            [yY]|[yY][eE][sS]) ;;
            *) info "Install aborted."; exit 0 ;;
        esac
    fi

    info "Installing binary to $dst"
    cp "$src" "$dst" || die "Failed to copy binary to $dst"
    chmod +x "$dst"
    info "Binary installed."
}

# ---------------------------------------------------------------------------
# PATH check
# ---------------------------------------------------------------------------
check_path() {
    local in_path=false
    case ":${PATH}:" in
        *:"$INSTALL_PREFIX":*) in_path=true ;;
    esac

    if [[ "$in_path" != true ]]; then
        warn "$INSTALL_PREFIX is not in your PATH."
        warn "Add the following to your shell profile (e.g. ~/.bashrc or ~/.zshrc):"
        warn "  export PATH=\"$INSTALL_PREFIX:\$PATH\""
    fi
}

# ---------------------------------------------------------------------------
# Init
# ---------------------------------------------------------------------------
run_init() {
    local mimir_bin="$INSTALL_PREFIX/mimir"

    info "Running mimir init …"
    echo
    # Run directly so stdin/stdout remain attached to the terminal,
    # preserving interactive prompts (identity, systemd, etc.).
    "$mimir_bin" init
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    info "Mimir installer"
    echo

    build_release
    install_binary
    check_path
    echo
    run_init

    echo
    info "Install complete."
    echo "  Binary: $INSTALL_PREFIX/mimir"
    echo "  Config: \${XDG_CONFIG_HOME:-\$HOME/.config}/mimir/config.toml"
}

main "$@"
