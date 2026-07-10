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

# Every workspace member (root Cargo.toml) must appear in exactly one of the
# two lists below — `measure` verifies this, so a crate added, split out
# (#178), or renamed fails here until it is classified. Measurement stays per
# crate, never --workspace.
COV_PACKAGES=(
	pallet-storage-provider
	pallet-drive-registry
	pallet-s3-registry
	storage-provider-node
	provider-negotiation
)

# Not measured, with the reason per crate.
COV_SKIP_PACKAGES=(
	storage-client # chain-bound SDK; integration tests self-skip without a live chain
	file-system-client # chain-bound layer-1 SDK
	s3-client # chain-bound layer-1 SDK
	storage-primitives # pure types; enters reports only via dep graph (see COV_IGNORE)
	file-system-primitives # pure types; enters reports only via dep graph (see COV_IGNORE)
	s3-primitives # pure types; enters reports only via dep graph (see COV_IGNORE)
	storage-parachain-runtime # exercised out of process (zombienet e2e)
	storage-paseo-runtime # exercised out of process (zombienet e2e)
	pallet-storage-provider-precompile # exercised out of process (`just sc-demo`)
	pallet-drive-registry-precompile # exercised out of process (`just sc-demo`)
	pallet-s3-registry-precompile # exercised out of process (`just sc-demo`)
)

# Fail when the two lists and the workspace members drift apart: every member
# classified, no stale or duplicated entries.
verify_classification() {
	local members classified drift
	members=$(cargo metadata --no-deps --format-version 1 |
		python3 -c 'import json,sys; print("\n".join(p["name"] for p in json.load(sys.stdin)["packages"]))' |
		sort)
	classified=$(printf '%s\n' "${COV_PACKAGES[@]}" "${COV_SKIP_PACKAGES[@]}" | sort)
	drift=$(diff <(echo "$members") <(echo "$classified") || true)
	if [ -n "$drift" ]; then
		{
			echo "error: coverage classification out of sync with the workspace."
			echo "Each root Cargo.toml member must be in COV_PACKAGES (measured) or"
			echo "COV_SKIP_PACKAGES (skipped, with a reason) in scripts/coverage.sh:"
			echo "$drift" | sed -e 's/^</  unclassified member: /' -e 's/^>/  stale or duplicated entry:/' -e '/^[0-9-]/d'
		} >&2
		exit 1
	fi
}

# Exclusions: only code that structurally cannot execute in this run —
# "untested but testable" code stays measured so the gate pushes for tests.
# The coordinators (checkpoint_coordinator, replica_sync_coordinator,
# challenge_responder) are deliberately NOT here: they abstract chain access
# behind traits and are covered by the mock-backed tests/coordinators/ suite.
# Groups: toolchain/vendored; generated code + test scaffolding ("bechmarking"
# is a real file in pallet-registry); primitives crates (linked in, own tests
# not run here); chain-access layer (needs a live chain); client SDK crates
# (in provider-node's dep graph until #277, never executed here);
# replica_sync.rs (no chain-client trait of its own, exercised only
# indirectly through replica_sync_coordinator — measure it once it is
# directly testable); binary entry points.
COV_IGNORE='(/\.cargo/|/rustc/|weights\.rs|runtime_api\.rs|mock\.rs|benchmarking\.rs|bechmarking\.rs|/primitives/|subxt_client\.rs|_subxt\.rs|client/src/|client/tests/|src/replica_sync\.rs|src/main\.rs|src/cli\.rs|src/command\.rs)'

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
	verify_classification

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
