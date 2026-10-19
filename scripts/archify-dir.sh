#!/bin/bash
#
# Prints the archify directory used by build-docs-site.sh and verify-archify.sh:
# $ARCHIFY_DIR if set, else ~/.claude/skills/archify if it exists, else
# ~/.cache/archify (the default install location; it may not exist yet).
#
# Usage: scripts/archify-dir.sh

set -euo pipefail

if [[ -n "${ARCHIFY_DIR:-}" ]]; then
  echo "$ARCHIFY_DIR"
elif [[ -d "$HOME/.claude/skills/archify" ]]; then
  echo "$HOME/.claude/skills/archify"
else
  echo "$HOME/.cache/archify"
fi
