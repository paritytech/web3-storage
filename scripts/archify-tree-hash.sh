#!/bin/bash
#
# Prints the git tree hash of a directory: the same value as
# `git rev-parse <commit>:archify` when the directory holds exactly that tree,
# including file modes and symlinks. Used by build-docs-site.sh and
# verify-archify.sh to compare an archify copy with the pinned commit.
#
# Usage: scripts/archify-tree-hash.sh <dir>

set -euo pipefail

dir=$(cd "$1" && pwd -P)
gitdir=$(mktemp -d)
trap 'rm -rf "$gitdir"' EXIT

git -C "$gitdir" init -q
# Ignore the user's git config so the hash depends on the files only.
GIT_DIR="$gitdir/.git" GIT_WORK_TREE="$dir" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
  git -c core.autocrlf=false -c core.attributesFile=/dev/null add -A -f :/
GIT_DIR="$gitdir/.git" git write-tree
