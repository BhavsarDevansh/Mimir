#!/usr/bin/env bash
set -euo pipefail

# Mimir uninstall script
# Usage: uninstall.sh [options]
#   -y, --yes       Full uninstall without confirmation (data + config + service)
#   -d, --data      Remove data directory (~/.local/share/mimir)
#   -c, --config    Remove config directory (~/.config/mimir)
#   -h, --help      Show this help message

SCRIPT_NAME="${0##*/}"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
CONFIRM="prompt"
REMOVE_DATA=false
REMOVE_CONFIG=false

# ---------------------------------------------------------------------------
# Paths (resolved via dirs conventions; XDG vars honoured)
# ---------------------------------------------------------------------------
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/mimir"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/mimir"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/mimir"
SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
SERVICE_NAME="mimir"
SERVICE_FILE="$SYSTEMD_USER_DIR/$SERVICE_NAME.service"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf '\033[1;34m[INFO]\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m[WARN]\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31m[ERROR]\033[0m %s\n' "$*" &&2; }
die()   { error "$*"; exit 1; }

usage() {
    cat <<USAGE
Mimir uninstall script

Usage:
  $SCRIPT_NAME [options]

Options:
  -y, --yes      Full uninstall without confirmation (implies --data and --config)
  -d, --data     Remove the data directory ($DATA_DIR)
  -c, --config   Remove the config directory ($CONFIG_DIR)
  -h, --help     Show this help message

With no options the script stops the systemd service (if present) and then
interactively asks whether to delete the data, config, and cache directories.
USAGE
}

# ---------------------------------------------------------------------------
# Parse args
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        -y|--yes)
            CONFIRM="auto"
            REMOVE_DATA=true
            REMOVE_CONFIG=true
            shift
            ;;
        -d|--data)
            REMOVE_DATA=true
            shift
            ;;
        -c|--config)
            REMOVE_CONFIG=true
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
# systemd service handling
# ---------------------------------------------------------------------------
remove_systemd_service() {
    if [[ ! -f "$SERVICE_FILE" ]]; then
        info "No systemd user service found at $SERVICE_FILE"
        return 0
    fi

    info "Found systemd user service: $SERVICE_FILE"

    if systemctl --user is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
        info "Stopping $SERVICE_NAME user service …"
        systemctl --user stop "$SERVICE_NAME" || warn "Failed to stop $SERVICE_NAME"
    else
        info "$SERVICE_NAME user service is not running"
    fi

    if systemctl --user is-enabled --quiet "$SERVICE_NAME" 2>/dev/null; then
        info "Disabling $SERVICE_NAME user service …"
        systemctl --user disable "$SERVICE_NAME" || warn "Failed to disable $SERVICE_NAME"
    fi

    info "Removing service file: $SERVICE_FILE"
    rm -f "$SERVICE_FILE"

    info "Reloading systemd user daemon …"
    systemctl --user daemon-reload || warn "systemctl daemon-reload failed"

    info "systemd user service removed"
}

# ---------------------------------------------------------------------------
# Directory removal
# ---------------------------------------------------------------------------
remove_dir() {
    local label="$1"
    local dir="$2"

    if [[ ! -e "$dir" ]]; then
        info "$label not found at $dir — nothing to remove"
        return 0
    fi

    info "Removing $label: $dir"
    rm -rf "$dir"
    info "$label removed"
}

# ---------------------------------------------------------------------------
# Confirmation prompt
# ---------------------------------------------------------------------------
prompt_confirm() {
    local msg="$1"
    local reply
    while true; do
        printf '%s [y/N]: ' "$msg"
        read -r reply
        case "$reply" in
            [yY]|[yY][eE][sS]) return 0 ;;
            [nN]|[nN][oO]|"") return 1 ;;
            *) echo "Please answer yes or no." ;;
        esac
    done
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    info "Mimir uninstall"
    echo

    # Determine what to remove
    local do_data=false
    local do_config=false
    local do_cache=false

    if [[ "$CONFIRM" == "auto" ]]; then
        do_data=true
        do_config=true
        do_cache=true
    else
        # --data was passed → remove data without asking
        if [[ "$REMOVE_DATA" == true ]]; then
            do_data=true
        else
            prompt_confirm "Remove data directory ($DATA_DIR)?" && do_data=true
        fi

        # --config was passed → remove config without asking
        if [[ "$REMOVE_CONFIG" == true ]]; then
            do_config=true
        else
            prompt_confirm "Remove config directory ($CONFIG_DIR)?" && do_config=true
        fi

        # Cache is always asked (no dedicated flag)
        prompt_confirm "Remove cache directory ($CACHE_DIR)?" && do_cache=true
    fi

    # Summary
    echo
    echo "Summary of actions:"
    echo "  • Stop and remove systemd user service (if present)"
    [[ "$do_data"   == true ]] && echo "  • Remove data directory:   $DATA_DIR"
    [[ "$do_config" == true ]] && echo "  • Remove config directory: $CONFIG_DIR"
    [[ "$do_cache"  == true ]] && echo "  • Remove cache directory:  $CACHE_DIR"
    echo

    if [[ "$CONFIRM" == "prompt" ]]; then
        prompt_confirm "Proceed with uninstall?" || {
            info "Uninstall aborted."
            exit 0
        }
    fi

    remove_systemd_service
    [[ "$do_data"   == true ]] && remove_dir "Data directory" "$DATA_DIR"
    [[ "$do_config" == true ]] && remove_dir "Config directory" "$CONFIG_DIR"
    [[ "$do_cache"  == true ]] && remove_dir "Cache directory" "$CACHE_DIR"

    echo
    info "Uninstall complete."
}

main "$@"
