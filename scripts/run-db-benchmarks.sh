#!/usr/bin/env bash
#
# Run the Issue #101 database engine benchmark harness and write raw JSON
# results under docs/design/database-evaluation/results/.
#
# Usage:
#   scripts/run-db-benchmarks.sh            # full run
#   scripts/run-db-benchmarks.sh --quick    # fast smoke run (tiny sizes)
#
#   DB_BENCH_OUTPUT=my-pass.json scripts/run-db-benchmarks.sh
#                                           # record a new pass without
#                                           # overwriting an existing one
#
# The reports under docs/design/database-evaluation/ are written from these
# JSON files. Re-run on representative target hardware before committing to an
# engine — absolute numbers are hardware-dependent; the relative ranking is the
# deliverable.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${REPO_ROOT}/docs/design/database-evaluation/results"
# Scratch lives on a disk-backed filesystem by default, not /tmp. On hosts where
# /tmp is a small tmpfs the matrix does not fit: the large-value scenarios hold
# ~1.5 GiB per engine before amplification, and sled's 3.2x pushed a 3.9 GiB
# tmpfs to ENOSPC. tmpfs also holds the data in the same RAM the engines need,
# which is what OOM-killed sled in earlier passes. Override with DB_BENCH_WORKDIR.
WORKDIR="${DB_BENCH_WORKDIR:-/var/tmp/db-bench}"
SEED="${DB_BENCH_SEED:-13371337}"

EXTRA_ARGS=()
if [[ "${1:-}" == "--quick" ]]; then
	EXTRA_ARGS+=("--quick")
	shift || true
fi
EXTRA_ARGS+=("$@")

mkdir -p "${OUT_DIR}"

# db-bench is its own workspace (excluded from the root one), so it is built by
# manifest path and its artifacts land under its own target directory.
BENCH_MANIFEST="${REPO_ROOT}/benchmarks/db-bench/Cargo.toml"

echo ">> Building db-bench (release) ..."
cargo build --release --manifest-path "${BENCH_MANIFEST}"

BIN="${REPO_ROOT}/benchmarks/db-bench/target/release/db-bench"

# One process per engine, merged afterwards. The scratch filesystem is a small
# tmpfs and each engine's scenarios peak at well over a gigabyte, so running the
# whole matrix in a single process exhausts it (ENOSPC). A process per engine
# also means one engine dying cannot take the other results down with it.
ENGINES=(sled sqlite redb rocksdb lmdb mdbx jammdb paritydb)
PARTIAL_DIR="${WORKDIR}/partials"
rm -rf "${PARTIAL_DIR}"
mkdir -p "${PARTIAL_DIR}"

for engine in "${ENGINES[@]}"; do
	echo ">> Running Storage Provider benchmarks: ${engine} ..."
	rm -rf "${WORKDIR}/storage"
	"${BIN}" --engine "${engine}" --seed "${SEED}" \
		--work-directory "${WORKDIR}/storage" \
		--output "${PARTIAL_DIR}/${engine}.json" "${EXTRA_ARGS[@]}" || {
		echo "!! ${engine} failed; continuing with the remaining engines" >&2
		rm -f "${PARTIAL_DIR}/${engine}.json"
	}
done

# Each measurement pass is kept in its own file: the reports cite them
# individually and explicitly do not compare across them, so overwriting the
# canonical storage-provider.json would orphan the tables written from it.
# Override the name to record a new pass.
OUT_FILE="${DB_BENCH_OUTPUT:-storage-provider.json}"

echo ">> Merging per-engine results into ${OUT_FILE} ..."
python3 "${REPO_ROOT}/scripts/merge-db-bench-results.py" \
	"${PARTIAL_DIR}" "${OUT_DIR}/${OUT_FILE}"
rm -rf "${WORKDIR}"

echo ">> Done. Results in ${OUT_DIR}:"
ls -1 "${OUT_DIR}"/*.json
