#!/usr/bin/env bash
#
# Run the Issue #101 database engine benchmark harness and write raw JSON
# results under docs/design/database-evaluation/results/.
#
# Usage:
#   scripts/run-db-benchmarks.sh            # full run, both components
#   scripts/run-db-benchmarks.sh --quick    # fast smoke run (tiny sizes)
#
# The reports under docs/design/database-evaluation/ are written from these
# JSON files. Re-run on representative target hardware before committing to an
# engine — absolute numbers are hardware-dependent; the relative ranking is the
# deliverable.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${REPO_ROOT}/docs/design/database-evaluation/results"
WORKDIR="${DB_BENCH_WORKDIR:-/tmp/db-bench}"
SEED="${DB_BENCH_SEED:-13371337}"

EXTRA_ARGS=()
if [[ "${1:-}" == "--quick" ]]; then
	EXTRA_ARGS+=("--quick")
	shift || true
fi
EXTRA_ARGS+=("$@")

mkdir -p "${OUT_DIR}"

echo ">> Building db-bench (release) ..."
cargo build -p db-bench --release

BIN="${REPO_ROOT}/target/release/db-bench"

echo ">> Running Storage Provider benchmarks (sled vs sqlite vs rocksdb) ..."
"${BIN}" --component storage --seed "${SEED}" --work-directory "${WORKDIR}/storage" \
	--output "${OUT_DIR}/storage-provider.json" "${EXTRA_ARGS[@]}"

echo ">> Running Blockchain Node benchmarks (rocksdb vs paritydb) ..."
"${BIN}" --component blockchain --seed "${SEED}" --work-directory "${WORKDIR}/blockchain" \
	--output "${OUT_DIR}/blockchain-provider.json" "${EXTRA_ARGS[@]}"

echo ">> Done. Results in ${OUT_DIR}:"
ls -1 "${OUT_DIR}"/*.json
