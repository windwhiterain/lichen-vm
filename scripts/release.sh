#!/usr/bin/env bash
#
# release.sh — trigger the GitHub Actions `release-lichen-toolchain` workflow,
# which builds the prebuilt toolchain **on CI** (all four host triples) and
# publishes it as a GitHub release tagged at the commit. The Zed extension (and
# `lichen install` / `lichen path`) download those assets, so a fresh machine
# boots without a local build.
#
# The build itself lives in `.github/workflows/release-lichen.yml`; this script
# is just a thin `gh` wrapper around it (no local Rust build here).
#
# Usage:
#   scripts/release.sh                        # trigger CI at the **current branch**
#   scripts/release.sh --ref <branch|commit>  # trigger CI at a specific ref
#   scripts/release.sh --watch                # also wait for the run to finish
#
# Requires: gh (https://cli.github.com), authenticated (`gh auth login`).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

REF=""
WATCH=0
while [ $# -gt 0 ]; do
  case "$1" in
    --ref) REF="${2:?--ref requires a branch or commit}"; shift 2 ;;
    --watch) WATCH=1; shift ;;
    --) shift; break ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

# Default the dispatch ref to the repo's **current branch**, so a branch rename
# (the old `v1` became `dev`) doesn't leave a stale hardcoded ref that GitHub
# rejects with "No ref found for: <ref>".  Fall back to origin's default branch
# on a detached HEAD.
if [ -z "$REF" ]; then
  REF="$(git -C "$ROOT" rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
  if [ -z "$REF" ] || [ "$REF" = "HEAD" ]; then
    REF="$(git -C "$ROOT" symbolic-ref --quiet --short refs/remotes/origin/HEAD \
            2>/dev/null || echo "main")"
    REF="${REF#origin/}"
  fi
fi

WORKFLOW="release-lichen.yml"

say()  { printf '\n== %s ==\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

command -v gh >/dev/null || fail "gh is required (https://cli.github.com)"
gh auth status >/dev/null 2>&1 || fail "gh is not authenticated — run \`gh auth login\` first"

say "triggering CI release workflow: gh workflow run $WORKFLOW --ref $REF"
gh workflow run "$WORKFLOW" --ref "$REF" \
  || fail "gh workflow run failed (check the workflow name, and that $REF is pushed to origin)"
say "run started. Monitor with:  gh run list --workflow=$WORKFLOW"
say "it publishes a release tagged at \`github.sha\` (the commit $REF points to) with the"
say "assets \`lichen-<host-target>[.exe]\`, \`lichen-compiler-<...>\`, \`lichen-language-server-<...>\`."

if [ "$WATCH" -eq 1 ]; then
  say "watching the run to completion..."
  id="$(gh run list --workflow="$WORKFLOW" --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
  if [ -n "$id" ]; then
    gh run watch "$id" --exit-status \
      || fail "the CI release run failed (see \`gh run list --workflow=$WORKFLOW\`)"
  else
    say "could not resolve the run id; check manually with \`gh run list --workflow=$WORKFLOW\`"
  fi
  commit="$(git -C "$ROOT" rev-parse "$REF" 2>/dev/null || echo "$REF")"
  say "published: https://github.com/windwhiterain/lichen-vm/releases/tag/$commit"
fi
