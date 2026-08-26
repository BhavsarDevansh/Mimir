#!/usr/bin/env bash
set -euo pipefail

# Regression guard for the release helper (scripts/new-release.sh): version
# detection, changelog section extraction, tag-existence checks, and the
# dry-run validation paths must behave deterministically so a release never
# publishes with the wrong tag, body, or version.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

source "$SCRIPT_DIR/new-release.sh"

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/Cargo.toml" <<'EOF'
[workspace.package]
version = "9.9.9"
EOF

cat > "$TMP/CHANGELOG.md" <<'EOF'
# Changelog

## [9.9.9] — 2099-01-01

### Feature: fixture section

- first bullet
- second bullet

## [9.9.8] — 2099-01-01

### Fix: older fixture section
EOF

# detect_version reads the workspace package version.
got="$(detect_version "$TMP/Cargo.toml")"
[[ "$got" == "9.9.9" ]] || fail "detect_version returned '$got', want 9.9.9"

# extract_section returns exactly the requested section.
got="$(extract_section "$TMP/CHANGELOG.md" 9.9.9)"
[[ "$got" == *"first bullet"* ]] || fail "extract_section omitted the section body"
[[ "$got" == *"second bullet"* ]] || fail "extract_section truncated the section body"
[[ "$got" != *"older fixture section"* ]] || fail "extract_section leaked the next section"

# tag_exists reflects local tags in the given repository.
REPO="$TMP/repo"
git init -q "$REPO"
git -C "$REPO" config user.email test@example.com
git -C "$REPO" config user.name Test
git -C "$REPO" commit -q --allow-empty -m init
tag_exists "$REPO" 9.9.9 && fail "tag_exists reported a missing tag as present"
git -C "$REPO" tag v9.9.9
tag_exists "$REPO" 9.9.9 || fail "tag_exists reported an existing tag as missing"

# Dry run refuses a version with no changelog section.
if CHANGELOG_FILE="$TMP/CHANGELOG.md" CARGO_MANIFEST="$TMP/Cargo.toml" \
    "$PROJECT_ROOT/scripts/new-release.sh" --dry-run --version 1.0.0 >/dev/null 2>&1; then
    fail "dry run must fail when the changelog section is missing"
fi

# Dry run rejects malformed versions.
if CHANGELOG_FILE="$TMP/CHANGELOG.md" CARGO_MANIFEST="$TMP/Cargo.toml" \
    "$PROJECT_ROOT/scripts/new-release.sh" --dry-run --version abc >/dev/null 2>&1; then
    fail "dry run accepted a malformed version"
fi

# Dry run rejects duplicate version arguments.
if CHANGELOG_FILE="$TMP/CHANGELOG.md" CARGO_MANIFEST="$TMP/Cargo.toml" \
    "$PROJECT_ROOT/scripts/new-release.sh" --dry-run --version 9.9.9 9.9.9 >/dev/null 2>&1; then
    fail "dry run accepted duplicate version arguments"
fi

# Dry run prints the plan and does not create the tag.
out="$(CHANGELOG_FILE="$TMP/CHANGELOG.md" CARGO_MANIFEST="$TMP/Cargo.toml" \
    "$PROJECT_ROOT/scripts/new-release.sh" --dry-run --version 9.9.9 2>&1)"
[[ "$out" == *"9.9.9"* ]] || fail "dry run output does not mention the version"
[[ "$out" == *"Dry run"* ]] || fail "dry run output does not say it is a dry run"
tag_exists "$PROJECT_ROOT" 9.9.9 && fail "dry run created a tag"

printf 'new-release_test.sh: all checks passed\n'
