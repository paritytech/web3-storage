#!/usr/bin/env bash
set -euo pipefail
gh release view --repo "$GITHUB_REPOSITORY" --json body --jq .body |
	sed -nE 's/.*Core Version: +[^ ]+-([0-9]+) .*/\1/p' | head -n1
