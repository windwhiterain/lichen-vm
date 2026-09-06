#!/usr/bin/env bash
#
# publish-toolchain.sh — build the prebuilt toolchain and publish it to a GitHub
# release with `gh`, so the Zed extension (and `lichen install` / `liche path`)
# can download it.  The release is tagged at the current commit SHA, which is the
# `LICHEN_BUILD_COMMIT` the built binaries embed, keeping toolchain and package
# manager at the same revision.
#
# This builds only the **current host** triple (a dev machine).  To publish all
# four host targets, run the GitHub Actions workflow instead:
#
#     gh workflow run release-lichen.yml --ref <commit> && gh run watch
#
# (the workflow builds the matrix and creates the release tagged at that commit).
#
# Usage:
#   scripts/publish-toolchain.sh            # build current host + gh release
#   scripts/publish-toolchain.sh --dry-run  # print the plans, do not run
#
# Requires: cargo, git, gh (authenticated: `gh auth status`).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DRY=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

# --- the host triple + asset suffix, matching toolchain::host_target/asset_name ---
uname_s="$(uname -s 2>/dev/null || echo unknown)"
case "$uname_s" in
  MINGW*|MSYS*|CYGWIN*) os=windows; exe=".exe" ;;
  Darwin)                os=macos;   exe="" ;;
  Linux)                 os=linux;   exe="" ;;
  *) echo "unsupported OS: $uname_s" >&2; exit 2 ;;
esac
arch="$(uname -m 2>/dev/null || echo unknown)"
case "$arch" in
  x86_64|amd64)           a=x86_64 ;;
  aarch64|arm64)          a=aarch64 ;;
  *) echo "unsupported arch: $arch" >&2; exit 2 ;;
esac
case "$os:$a" in
  windows:x86_64)  triple="x86_64-pc-windows-msvc" ;;
  windows:aarch64) triple="aarch64-pc-windows-msvc" ;;
  macos:aarch64)   triple="aarch64-apple-darwin" ;;
  macos:x86_64)    triple="x86_64-apple-darwin" ;;
  linux:x86_64)    triple="x86_64-unknown-linux-gnu" ;;
  linux:aarch64)   triple="aarch64-unknown-linux-gnu" ;;
  *) echo "unsupported $(uname -s) $(uname -m) target" >&2; exit 2 ;;
esac

commit="$(git -C "$ROOT" rev-parse HEAD)"
tag="$commit"   # the release tag is the commit SHA (as the workflow does)

say()  { printf '\n== %s ==\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

command -v gh >/dev/null || fail "gh is required (https://cli.github.com)"
gh auth status >/dev/null 2>&1 || fail "gh is not authenticated — run \`gh auth login\` first"

say "building toolchain for $triple at $commit"
cargo build --release --locked --manifest-path "$ROOT/Cargo.toml" \
  -p lichen-package -p lichen-language -p lichen-language-server \
  || fail "cargo build --release"

DIST="$ROOT/dist-toolchain"
mkdir -p "$DIST"
cp "$ROOT/target/release/lichen${exe}"                 "$DIST/lichen-${triple}${exe}"
cp "$ROOT/target/release/lichen-compiler${exe}"        "$DIST/lichen-compiler-${triple}${exe}"
cp "$ROOT/target/release/lichen-language-server${exe}" "$DIST/lichen-language-server-${triple}${exe}"
echo "staged:"
ls -1 "$DIST"

if [ "$DRY" -eq 1 ]; then
  cat <<EOF

would run:
  gh release create "$tag" "$DIST"/* --prerelease \
    --title "lichen toolchain $commit" \
    --notes "prebuilt toolchain for the lichen Zed extension (see docs/notes/venv-test.md)."
EOF
  exit 0
fi

say "publishing release at $tag"
gh release create "$tag" "$DIST"/* --prerelease \
  --title "lichen toolchain $commit" \
  --notes "prebuilt toolchain for the lichen Zed extension." \
  || fail "gh release create failed (a release at this tag may already exist)"
say "published: https://github.com/windwhiterain/lichen-vm/releases/tag/$tag"

rm -rf "$DIST"
