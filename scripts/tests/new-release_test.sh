#!/usr/bin/env bash
set -euo pipefail

# Regression guard for the release helper (scripts/new-release.sh): version
# detection, literal changelog section extraction, local/remote tag checks,
# working-tree and `gh` validation, resumable publication, and the dry-run
# paths must behave deterministically so a release never publishes with the
# wrong tag, body, or version.

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

# extract_section matches the heading as literal text: dots in the version
# must not act as wildcards, and a longer version must not shadow it.
cat > "$TMP/CHANGELOG-wild.md" <<'EOF'
# Changelog

## [0x152y2] — 2099-01-01

### Trap section

- wrong notes

## [0.152.2] — 2099-01-01

### Literal section

- correct notes

## [0.152.20] — 2099-01-01

### Later section

- later notes
EOF

got="$(extract_section "$TMP/CHANGELOG-wild.md" 0.152.2)"
[[ "$got" == *"correct notes"* ]] || fail "extract_section must match the literal heading"
[[ "$got" != *"wrong notes"* ]] || fail "extract_section matched a wildcard-substitution heading"
[[ "$got" != *"later notes"* ]] || fail "extract_section leaked the next section"

# tag_exists reflects local tags in the given repository.
REPO="$TMP/repo"
git init -q "$REPO"
git -C "$REPO" config user.email test@example.com
git -C "$REPO" config user.name Test
git -C "$REPO" commit -q --allow-empty -m init
tag_exists "$REPO" 9.9.9 && fail "tag_exists reported a missing tag as present"
git -C "$REPO" tag v9.9.9
tag_exists "$REPO" 9.9.9 || fail "tag_exists reported an existing tag as missing"

# remote_tag_target reflects origin tags: the peeled commit for annotated
# tags, the raw commit for lightweight tags, and nothing when origin has none.
BARE="$TMP/remote-origin.git"
git init -q --bare "$BARE"
git -C "$REPO" remote add origin "$BARE"
[[ -z "$(remote_tag_target "$REPO" v9.9.7)" ]] || \
    fail "remote_tag_target reported a missing remote tag as present"
git -C "$REPO" push -q origin v9.9.9
[[ "$(remote_tag_target "$REPO" v9.9.9)" == "$(git -C "$REPO" rev-parse HEAD)" ]] || \
    fail "remote_tag_target missed the lightweight tag"
git -C "$REPO" tag -a v9.9.8 -m "Release 9.9.8"
git -C "$REPO" push -q origin v9.9.8
[[ "$(remote_tag_target "$REPO" v9.9.8)" == "$(git -C "$REPO" rev-parse "refs/tags/v9.9.8^{}")" ]] || \
    fail "remote_tag_target missed the annotated tag"
git -C "$REPO" remote remove origin

# Sandbox fixture: a hermetic repo copy of the release script plus a bare
# origin, so main() runs against a throwaway repository.
make_sandbox() {
    local sandbox="$1" origin="$2"
    mkdir -p "$sandbox/scripts"
    cp "$SCRIPT_DIR/new-release.sh" "$sandbox/scripts/new-release.sh"
    chmod +x "$sandbox/scripts/new-release.sh"
    cp "$TMP/Cargo.toml" "$sandbox/Cargo.toml"
    cp "$TMP/CHANGELOG.md" "$sandbox/CHANGELOG.md"
    git init -q "$sandbox"
    git -C "$sandbox" config user.email test@example.com
    git -C "$sandbox" config user.name Test
    git -C "$sandbox" commit -q --allow-empty -m init
    git -C "$sandbox" branch -M main
    git init -q --bare "$origin"
    git -C "$sandbox" remote add origin "$origin"
    git -C "$sandbox" add -A
    git -C "$sandbox" commit -q -m fixtures
    git -C "$sandbox" push -q origin main
}

# gh test double: never touches the real GitHub CLI. `release view` reports a
# missing release, and `release create` fails once (simulating a transient
# publication failure) before succeeding, tracked via GH_STATE_FILE.
GH_SHIM="$TMP/gh-shim"
mkdir -p "$GH_SHIM"
cat > "$GH_SHIM/gh" <<'EOF'
#!/usr/bin/env bash
STATE_FILE="${GH_STATE_FILE:-/nonexistent-gh-state}"
case "$1" in
    auth)
        exit 0
        ;;
    release)
        case "$2" in
            view)
                exit 1
                ;;
            create)
                if [[ ! -f "$STATE_FILE" ]]; then
                    touch "$STATE_FILE"
                    echo "simulated publication failure" >&2
                    exit 1
                fi
                echo "https://example.com/releases/$3"
                ;;
        esac
        ;;
esac
exit 0
EOF
chmod +x "$GH_SHIM/gh"
export PATH="$GH_SHIM:$PATH"

# A tag already on origin at a different commit aborts before any local tag
# is created or GitHub is touched.
SANDBOX="$TMP/sandbox-remote"
make_sandbox "$SANDBOX" "$TMP/origin-remote.git"
git -C "$SANDBOX" tag v9.9.9
git -C "$SANDBOX" push -q origin v9.9.9
git -C "$SANDBOX" tag -d v9.9.9
git -C "$SANDBOX" commit -q --allow-empty -m newer
out="$("$SANDBOX/scripts/new-release.sh" --version 9.9.9 2>&1)" && \
    fail "release must refuse when origin already has the tag at another commit"
[[ "$out" == *"Origin already has tag"* ]] || fail "remote-tag conflict lacks a clear message: $out"
tag_exists "$SANDBOX" 9.9.9 && fail "conflicting release created a local tag"

# A dirty working tree fails a real release but only warns under --dry-run.
SANDBOX="$TMP/sandbox-dirty"
make_sandbox "$SANDBOX" "$TMP/origin-dirty.git"
echo dirty > "$SANDBOX/uncommitted.txt"
out="$("$SANDBOX/scripts/new-release.sh" --version 9.9.9 2>&1)" && \
    fail "real release must refuse a dirty working tree"
[[ "$out" == *"Working tree is not clean"* ]] || fail "dirty-tree error lacks a clear message: $out"
tag_exists "$SANDBOX" 9.9.9 && fail "dirty-tree release created a tag"
out="$("$SANDBOX/scripts/new-release.sh" --dry-run --version 9.9.9 2>&1)" || \
    fail "dry run must tolerate a dirty working tree"
[[ "$out" == *"Working tree is not clean"* ]] || fail "dry run must warn about a dirty working tree"

# An unauthenticated gh aborts before the tag is created.
SANDBOX="$TMP/sandbox-noauth"
make_sandbox "$SANDBOX" "$TMP/origin-noauth.git"
NOAUTH_SHIM="$TMP/gh-noauth-shim"
mkdir -p "$NOAUTH_SHIM"
cat > "$NOAUTH_SHIM/gh" <<'EOF'
#!/usr/bin/env bash
echo "gh: not authenticated" >&2
exit 1
EOF
chmod +x "$NOAUTH_SHIM/gh"
out="$(PATH="$NOAUTH_SHIM:$PATH" "$SANDBOX/scripts/new-release.sh" --version 9.9.9 2>&1)" && \
    fail "real release must refuse an unauthenticated gh"
[[ "$out" == *"Not authenticated"* ]] || fail "auth failure lacks a clear message: $out"
tag_exists "$SANDBOX" 9.9.9 && fail "release tagged before checking gh authentication"

# A failed `gh release create` leaves the tag published; the next invocation
# resumes and creates the missing release instead of aborting.
SANDBOX="$TMP/sandbox-resume"
make_sandbox "$SANDBOX" "$TMP/origin-resume.git"
out="$(GH_STATE_FILE="$TMP/gh-state" "$SANDBOX/scripts/new-release.sh" --version 9.9.9 2>&1)" && \
    fail "first publication must fail when gh release create fails"
tag_exists "$SANDBOX" 9.9.9 || fail "failed publication must keep the local tag"
[[ -n "$(git -C "$SANDBOX" ls-remote origin refs/tags/v9.9.9)" ]] || \
    fail "failed publication must keep the pushed remote tag"
out="$(GH_STATE_FILE="$TMP/gh-state" "$SANDBOX/scripts/new-release.sh" --version 9.9.9 2>&1)" || \
    fail "resume run must publish the missing release"
[[ "$out" == *"Released: https://example.com/releases/v9.9.9"* ]] || \
    fail "resume run did not create the release: $out"

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
