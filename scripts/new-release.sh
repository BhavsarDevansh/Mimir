#!/usr/bin/env bash
set -euo pipefail

# Mimir release script
# Publishes the current version as a GitHub release with no CI/CD: reads the
# version from the workspace manifest (or an explicit VERSION argument), takes
# the matching `## [VERSION]` section from CHANGELOG.md as the release body,
# creates an annotated tag, pushes it, and runs `gh release create`. A real
# release requires a clean working tree and an authenticated `gh` CLI; a failed
# publication can be retried and resumes from the already-pushed tag.
#
# Usage: new-release.sh [options] [VERSION]
#   VERSION        Version to release (default: workspace Cargo.toml version)
#   --version VER  Same as the positional VERSION argument
#   --dry-run      Validate and print the plan without tagging or publishing
#   -h, --help     Show this help message

SCRIPT_NAME="${0##*/}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_MANIFEST="${CARGO_MANIFEST:-$PROJECT_ROOT/Cargo.toml}"
CHANGELOG_FILE="${CHANGELOG_FILE:-$PROJECT_ROOT/CHANGELOG.md}"

DRY_RUN=false

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf '\033[1;34m[INFO]\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m[WARN]\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31m[ERROR]\033[0m %s\n' "$*" >&2; }
die()   { error "$*"; exit 1; }

usage() {
    cat <<'USAGE'
Mimir release script

Usage:
  new-release.sh [options] [VERSION]

Options:
  --version VER  Version to release (default: workspace Cargo.toml version)
  --dry-run      Print the release plan without tagging or publishing
  -h, --help     Show this help message

Environment:
  CARGO_MANIFEST  Workspace manifest to read the version from (default:
                  Cargo.toml at the repository root)
  CHANGELOG_FILE  Changelog with the release notes (default: CHANGELOG.md
                  at the repository root)

The script will:
  1. Read VERSION (argument or workspace manifest)
  2. Extract the `## [VERSION]` section from the changelog
  3. Validate local/remote tag state and a clean working tree
  4. Check `gh` authentication, then create and push the annotated tag
     vVERSION (skipped when already present)
  5. Create the GitHub release with the changelog section as its body
USAGE
}

# ---------------------------------------------------------------------------
# Pure helpers (unit-tested by scripts/tests/new-release_test.sh)
# ---------------------------------------------------------------------------
detect_version() {
    local manifest="$1"
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$manifest" | head -n 1
}

extract_section() {
    local changelog="$1" version="$2"
    awk -v version="$version" '
        index($0, "## [" version "]") == 1 { active = 1 }
        active && index($0, "## [") == 1 && index($0, "## [" version "]") != 1 { exit }
        active { print }
    ' "$changelog"
}

tag_exists() {
    local repo="$1" version="$2"
    git -C "$repo" rev-parse -q --verify "refs/tags/v$version" >/dev/null
}

remote_tag_target() {
    # Prints the commit origin's tag points at (empty when origin has no tag).
    local repo="$1" tag="$2"
    local peeled
    peeled="$(git -C "$repo" ls-remote origin "refs/tags/$tag^{}")"
    if [[ -n "$peeled" ]]; then
        printf '%s\n' "$peeled" | awk '{print $1}'
        return 0
    fi
    git -C "$repo" ls-remote origin "refs/tags/$tag" | awk '{print $1}'
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    local version=""
    local notes_file url
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            --version)
                [[ $# -ge 2 ]] || die "Missing value for --version"
                [[ -z "$version" ]] || die "Version given more than once"
                version="$2"
                shift 2
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            -*)
                die "Unknown option: $1"
                ;;
            *)
                [[ -z "$version" ]] || die "Version given more than once: $1"
                version="$1"
                shift
                ;;
        esac
    done

    if [[ -z "$version" ]]; then
        [[ -f "$CARGO_MANIFEST" ]] || die "Manifest not found: $CARGO_MANIFEST"
        version="$(detect_version "$CARGO_MANIFEST")"
    fi
    [[ -n "$version" ]] || die "Could not detect a version in $CARGO_MANIFEST"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "Invalid version: $version"

    [[ -f "$CHANGELOG_FILE" ]] || die "Changelog not found: $CHANGELOG_FILE"

    local notes
    notes="$(extract_section "$CHANGELOG_FILE" "$version")"
    [[ -n "$notes" ]] || die "No \`## [$version]\` section in $CHANGELOG_FILE"

    local tag="v$version"
    local head_sha tag_target remote_target
    head_sha="$(git -C "$PROJECT_ROOT" rev-parse HEAD)"
    tag_target=""
    if tag_exists "$PROJECT_ROOT" "$version"; then
        tag_target="$(git -C "$PROJECT_ROOT" rev-parse "refs/tags/$tag^{}")"
    fi
    remote_target="$(remote_tag_target "$PROJECT_ROOT" "$tag")"

    if [[ -n "$tag_target" && "$tag_target" != "$head_sha" ]]; then
        die "Tag $tag already exists locally at $tag_target - refusing to re-release"
    fi
    if [[ -n "$remote_target" && "$remote_target" != "$head_sha" ]]; then
        die "Origin already has tag $tag at $remote_target - refusing to re-release"
    fi

    if [[ -n "$(git -C "$PROJECT_ROOT" status --porcelain)" ]]; then
        if [[ "$DRY_RUN" == true ]]; then
            warn "Working tree is not clean; the release will tag the current HEAD"
        else
            die "Working tree is not clean; commit or stash changes before releasing"
        fi
    fi

    info "Version: $version"
    info "Tag:     $tag"
    info "Notes:   $(printf '%s' "$notes" | wc -l) lines from CHANGELOG.md"

    if [[ "$DRY_RUN" == true ]]; then
        info "Dry run - nothing was tagged or published"
        return 0
    fi

    command -v gh >/dev/null || die "GitHub CLI (gh) is required: install it and run gh auth login"
    gh auth status >/dev/null 2>&1 || die "Not authenticated with GitHub: run gh auth login"

    if [[ -z "$tag_target" ]]; then
        git -C "$PROJECT_ROOT" tag -a "$tag" -m "Release $version"
    fi
    if [[ -z "$remote_target" ]]; then
        git -C "$PROJECT_ROOT" push origin "$tag"
    fi

    notes_file="$(mktemp)"
    trap "rm -f '$notes_file'" EXIT
    printf '%s\n' "$notes" > "$notes_file"
    if gh release view "$tag" >/dev/null 2>&1; then
        info "Release $tag already exists - nothing to publish"
        return 0
    fi
    url="$(gh release create "$tag" --title "$tag" --notes-file "$notes_file")"
    info "Released: $url"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
