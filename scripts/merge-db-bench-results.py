#!/usr/bin/env python3
"""Merge per-engine db-bench result files into one results document.

The harness runs one process per engine (the scratch tmpfs cannot hold the whole
matrix at once), so this stitches the partial outputs back into the single JSON
file the reports are written from. Engine order follows the file names so the
merged output is deterministic.
"""

import json
import pathlib
import sys


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <partial-dir> <output-file>", file=sys.stderr)
        return 2

    partial_dir = pathlib.Path(sys.argv[1])
    output_path = pathlib.Path(sys.argv[2])

    partials = sorted(partial_dir.glob("*.json"))
    if not partials:
        print(f"no partial results found in {partial_dir}", file=sys.stderr)
        return 1

    meta = None
    records = []
    for path in partials:
        document = json.loads(path.read_text())
        # Every partial shares the seed/scale/host; keep the first and verify
        # the rest agree, so a mismatched run cannot be merged silently.
        if meta is None:
            meta = document["meta"]
        elif document["meta"] != meta:
            differing = {
                key: (meta.get(key), document["meta"].get(key))
                for key in set(meta) | set(document["meta"])
                if meta.get(key) != document["meta"].get(key)
            }
            print(f"refusing to merge {path.name}: meta differs {differing}", file=sys.stderr)
            return 1
        records.extend(document["records"])

    output_path.write_text(
        json.dumps({"meta": meta, "records": records}, indent=2) + "\n"
    )
    engines = sorted({record["engine"] for record in records})
    print(f"merged {len(records)} records from {len(partials)} engines: {', '.join(engines)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
