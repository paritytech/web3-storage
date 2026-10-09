#!/bin/bash
#
# Checks that docs/site/ was regenerated after the last change to docs/design/.
#
# docs/site/design.sha256 records the SHA-256 of every file under docs/design/
# that the site was last built from. This script fails when a design file was
# added, removed or edited since then. It checks the hashes only, not the site
# content.
#
# Usage:
#   scripts/check-docs-site.sh           # check (CI)
#   scripts/check-docs-site.sh --update  # record the current design docs,
#                                        # after updating docs/site/

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
export LC_ALL=C

RECORD=docs/site/design.sha256

design_hashes() {
  find docs/design -type f -print0 | sort -z | xargs -0 sha256sum
}

if [[ "${1:-}" == "--update" ]]; then
  design_hashes > "$RECORD"
  echo "Updated $RECORD"
  exit 0
fi

if diff -u "$RECORD" <(design_hashes); then
  echo "docs/site/ matches docs/design/."
  exit 0
fi

cat >&2 <<'EOF'

docs/design/ changed after docs/site/ was last updated.
Update the diagrams and the dApp guide in docs/site/ (Claude Code: run the
/generate-docs skill), then record the new design docs:

  scripts/check-docs-site.sh --update
EOF
exit 1
