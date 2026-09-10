#!/usr/bin/env bash
# Fail when a pallet declares an Error or Event variant that nothing in the
# pallet raises or emits.
#
# rustc cannot catch these: the enums are exported API, and FRAME's macros
# reference every variant (the error `as_str` match, the event derives), so
# `dead_code` never fires. A dead variant still costs a slot in the runtime
# metadata and a type in every client's generated bindings.
#
# Usage: scripts/check-dead-variants.sh   (run from anywhere in the repo)
set -euo pipefail
cd "$(dirname "$0")/.."

status=0
for lib in crates/pallets/*/src/lib.rs; do
  src_dir=$(dirname "$lib")
  # Production sources only: a variant used solely by tests or benchmarks is
  # still dead in the runtime.
  sources=$(find "$src_dir" -name '*.rs' \
    ! -path '*/tests/*' ! -name 'tests.rs' ! -name 'mock.rs' ! -name 'benchmarking.rs')

  for kind in error event; do
    # Variant names sit at 8-space indent directly inside the enum; fields
    # and nested braces are deeper, comments start with '/'.
    variants=$(awk -v attr="#[pallet::$kind]" '
      index($0, attr) { inblock = 1; next }
      inblock && /^    }$/ { exit }
      inblock && match($0, /^        [A-Z][A-Za-z0-9]*/) { print substr($0, 9, RLENGTH - 8) }
    ' "$lib")

    for v in $variants; do
      if [ "$kind" = error ]; then
        pattern="Error::(<T>::)?$v([^A-Za-z0-9_]|$)"
        verb="raised"
      else
        pattern="Event::$v([^A-Za-z0-9_]|$)"
        verb="emitted"
      fi
      # shellcheck disable=SC2086 # $sources is a whitespace-separated file list
      if ! grep -qE "$pattern" $sources; then
        echo "$lib: $kind variant \`$v\` is never $verb"
        status=1
      fi
    done
  done
done

if [ "$status" -ne 0 ]; then
  echo
  echo "Remove the variants above or wire them up. Removing one changes the"
  echo "runtime metadata: regenerate the subxt and PAPI bindings in the same PR."
fi
exit "$status"
