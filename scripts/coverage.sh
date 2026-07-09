#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Rust coverage measurement + patch gate — the single source of truth shared
# by CI (check.yml `coverage` job) and `just coverage`.
#
# Usage: scripts/coverage.sh {measure|gate|all}
# Env: COMPARE_BRANCH (default origin/dev), MIN_PATCH_COV (default 80)
# Outputs: lcov.info, coverage-summary.txt, coverage-modules.md,
# patch-coverage.md, target/llvm-cov-html/.

set -euo pipefail

# Stable number formatting and sort order regardless of the host locale.
export LC_ALL=C

# The measured module set — per crate, never --workspace. A crate not listed
# here is invisible to the gate, so every testable crate split out of
# provider-node (#178) must be added; the chain-touching node bin stays out.
# `cargo llvm-cov` fails loudly on unknown names, so renames can't silently
# zero the measurement.
COV_PACKAGES=(
	pallet-storage-provider
	pallet-drive-registry
	pallet-s3-registry
	storage-provider-node
)

# Exclusions: only code that structurally cannot execute in this run —
# "untested but testable" code stays measured so the gate pushes for tests.
# Groups: toolchain/vendored; generated code + test scaffolding ("bechmarking"
# is a real file in pallet-registry); primitives crates (linked in, own tests
# not run here); chain-access layer (needs a live chain); client SDK crates
# (in provider-node's dep graph until #277, never executed here); binary
# entry points.
COV_IGNORE='(/\.cargo/|/rustc/|weights\.rs|runtime_api\.rs|mock\.rs|benchmarking\.rs|bechmarking\.rs|/primitives/|subxt_client\.rs|_subxt\.rs|client/src/|client/tests/|src/main\.rs|src/cli\.rs|src/command\.rs)'

REPO_ROOT="${GITHUB_WORKSPACE:-$(git rev-parse --show-toplevel)}"
cd "$REPO_ROOT"

need() {
	command -v "$1" >/dev/null 2>&1 || {
		echo "error: '$1' not found — install it with: $2" >&2
		exit 1
	}
}

# Per-module table from a repo-relative lcov file, grouping by crate dir
# (path up to /src/ or /tests/) so it follows the #178 reorg without edits.
modules_table() {
	echo "| Module | Line coverage | Lines |"
	echo "|---|---|---|"
	awk '
		/^SF:/ { f = substr($0, 4); sub(/\/(src|tests)\/.*/, "", f); mod = f }
		/^LF:/ { lf[mod] += substr($0, 4) }
		/^LH:/ { lh[mod] += substr($0, 4) }
		END {
			for (m in lf) {
				pct = lf[m] ? 100 * lh[m] / lf[m] : 0
				printf "%s\t%.2f\t%d\t%d\n", m, pct, lh[m], lf[m]
			}
		}
	' "$1" | sort | awk -F'\t' '
		{ printf "| `%s` | %.2f%% | %d/%d |\n", $1, $2, $3, $4; th += $3; tf += $4 }
		END {
			pct = tf ? 100 * th / tf : 0
			printf "| **total** | **%.2f%%** | %d/%d |\n", pct, th, tf
		}
	'
}

measure() {
	need cargo-llvm-cov "cargo install cargo-llvm-cov"

	local pkg_flags=()
	local pkg
	for pkg in "${COV_PACKAGES[@]}"; do
		pkg_flags+=(-p "$pkg")
	done

	# One instrumented run of the measured modules' tests (lib + integration).
	# A build/test failure aborts here — no silent zero-coverage fallthrough.
	cargo llvm-cov --no-report "${pkg_flags[@]}"

	# Render the collected data in several formats without re-running tests.
	cargo llvm-cov report --lcov --output-path lcov.info --ignore-filename-regex "$COV_IGNORE"
	cargo llvm-cov report --html --output-dir target/llvm-cov-html --ignore-filename-regex "$COV_IGNORE"
	cargo llvm-cov report --ignore-filename-regex "$COV_IGNORE" >coverage-summary.txt

	if [ ! -s lcov.info ]; then
		echo "error: lcov.info is empty — coverage generation failed" >&2
		exit 1
	fi

	# Make lcov source paths repo-relative so diff-cover matches git paths.
	sed -i "s|SF:${REPO_ROOT}/|SF:|g" lcov.info

	modules_table lcov.info >coverage-modules.md
	echo
	cat coverage-modules.md
}

gate() {
	need diff-cover "python3 -m pip install 'diff-cover>=9,<10'"

	if [ ! -s lcov.info ]; then
		echo "error: lcov.info not found — run '$0 measure' first" >&2
		exit 1
	fi

	local compare_branch="${COMPARE_BRANCH:-origin/dev}"
	local min_patch_cov="${MIN_PATCH_COV:-80}"

	if ! git rev-parse -q --verify "$compare_branch" >/dev/null; then
		echo "error: '$compare_branch' not found — run: git fetch origin ${compare_branch#origin/}" >&2
		exit 1
	fi

	# Gate only on lines changed relative to the base; the report names the
	# exact uncovered lines.
	diff-cover lcov.info \
		--compare-branch "$compare_branch" \
		--fail-under "$min_patch_cov" \
		--show-uncovered \
		--markdown-report patch-coverage.md
}

case "${1:-all}" in
measure) measure ;;
gate) gate ;;
all)
	measure
	gate
	;;
*)
	echo "usage: $0 {measure|gate|all}" >&2
	exit 1
	;;
esac
