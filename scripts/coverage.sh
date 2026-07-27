#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Rust coverage measurement + patch gate — the single source of truth shared
# by CI (check.yml `coverage` job) and `just coverage`.
#
# Usage: scripts/coverage.sh {measure|gate|all}
# Env: COMPARE_BRANCH (default origin/dev), MIN_PATCH_COV (default 80)
# Outputs: lcov.info, lcov-integration.info, coverage-summary.txt,
# coverage-modules.md, coverage-integration.md, patch-coverage.md,
# target/llvm-cov-html/.

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
	storage-subxt # static codegen runtime bindings
	storage-indexers # chain-bound streams; needs a live chain to exercise
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
# The coordinators (replica_sync_coordinator, challenge_responder) are
# deliberately NOT here: they abstract chain access behind traits and are
# covered by the mock-backed tests/coordinators/ suite.
# Groups: toolchain/vendored; generated code + test scaffolding; primitives
# crates (linked in, own tests
# not run here); chain-access layer (needs a live chain); client SDK crates
# (in provider-node's dep graph until #277, never executed here);
# replica_sync.rs (no chain-client trait of its own, exercised only
# indirectly through replica_sync_coordinator — measure it once it is
# directly testable); binary entry points.
COV_IGNORE='(/\.cargo/|/rustc/|weights\.rs|runtime_api\.rs|mock\.rs|benchmarking\.rs|/primitives/|subxt_client\.rs|_subxt\.rs|clients/[^/]+/src/|clients/[^/]+/tests/|src/replica_sync\.rs|src/main\.rs|src/cli\.rs|src/command\.rs)'

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

	# Two views from one instrumented build, no test executed twice:
	# `report` renders every profraw accumulated so far and cannot select a
	# subset, so run integration targets first, render the integration-only
	# view, then run the remaining targets (--lib --bins covers exactly
	# what the single default-target invocation ran) and render the merged
	# view. The upfront clean keeps a local rerun's stale profraws out of
	# the first view. A build/test failure aborts here — no silent
	# zero-coverage fallthrough.
	cargo llvm-cov clean --profraw-only
	# --test '*' = every target under tests/ — picks up new suites
	# automatically, skips packages that have none. Invariant: at least one
	# measured crate must keep a tests/ target (today: provider-node) —
	# cargo fails the run if the glob matches nothing across the whole
	# selection, which is the right signal that this split needs a rethink.
	cargo llvm-cov --no-report "${pkg_flags[@]}" --test '*'
	cargo llvm-cov report --lcov --output-path lcov-integration.info --ignore-filename-regex "$COV_IGNORE"
	cargo llvm-cov --no-report "${pkg_flags[@]}" --lib --bins

	# Render the collected data in several formats without re-running tests.
	cargo llvm-cov report --lcov --output-path lcov.info --ignore-filename-regex "$COV_IGNORE"
	cargo llvm-cov report --html --output-dir target/llvm-cov-html --ignore-filename-regex "$COV_IGNORE"
	cargo llvm-cov report --ignore-filename-regex "$COV_IGNORE" >coverage-summary.txt

	if [ ! -s lcov.info ] || [ ! -s lcov-integration.info ]; then
		echo "error: lcov.info or lcov-integration.info is empty — coverage generation failed" >&2
		exit 1
	fi

	# Make lcov source paths repo-relative so diff-cover matches git paths.
	# -i.bak + rm stays portable across GNU and BSD/macOS sed.
	sed -i.bak "s|SF:${REPO_ROOT}/|SF:|g" lcov.info lcov-integration.info
	rm -f lcov.info.bak lcov-integration.info.bak

	modules_table lcov.info >coverage-modules.md

	# Diagnostic only, not gated: what integration tests alone reach.
	{
		echo "Line coverage reachable through integration tests alone —"
		echo "diagnostic only, no gate."
		echo
		modules_table lcov-integration.info
	} >coverage-integration.md

	echo
	cat coverage-modules.md
	echo
	echo "Integration-only coverage (public-API view):"
	echo
	cat coverage-integration.md
}

gate() {
	# Minimum version lives in .github/env (single source of truth, also
	# installed by CI), so local and CI gate behavior cannot silently
	# diverge — `--format <fmt>:<path>` only exists since 9.3.0.
	local min_dc installed
	min_dc=$(sed -n 's/^DIFF_COVER_MIN_VERSION=//p' .github/env)
	if [ -z "$min_dc" ]; then
		echo "error: DIFF_COVER_MIN_VERSION not set in .github/env" >&2
		exit 1
	fi

	need diff-cover "python3 -m pip install 'diff-cover>=${min_dc}'"

	installed=$(diff-cover --version 2>/dev/null | awk '{print $2; exit}')
	if [ "$(printf '%s\n' "$min_dc" "$installed" | sort -V | head -n1)" != "$min_dc" ]; then
		echo "error: diff-cover >= ${min_dc} required (found ${installed:-unknown}) — upgrade with: python3 -m pip install --upgrade 'diff-cover>=${min_dc}'" >&2
		exit 1
	fi

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
		--format markdown:patch-coverage.md
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
