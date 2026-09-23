#!/bin/bash
#
# Checks an archify commit for known risk patterns before anyone installs or
# updates it, and checks that the installed copy is the pinned commit.
#
# The pinned commit is in .claude/skills/generate-docs/archify.lock.
#
# Usage:
#   scripts/verify-archify.sh            # check the pinned commit and the installed copy
#   scripts/verify-archify.sh <commit>   # check a candidate commit before an update;
#                                        # writes the diff from the pinned commit for review
#
# Exit code 0 means every automated check passed. The checks search for known
# patterns only: code that builds names at run time, or any change inside an
# allowlisted file, passes them. They do not replace reading the diff.

set -euo pipefail
export LC_ALL=C

cd "$(git rev-parse --show-toplevel)"
root=$PWD
LOCK=.claude/skills/generate-docs/archify.lock
repo=$(sed -n 's/^repo=//p' "$LOCK")
pinned=$(sed -n 's/^commit=//p' "$LOCK")
pinned_tree=$(sed -n 's/^tree=//p' "$LOCK")
target=${1:-$pinned}
installed=${ARCHIFY_DIR:-$HOME/.claude/skills/archify}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git clone -q "$repo" "$work/repo"
git -C "$work/repo" cat-file -e "$target^{commit}"
target=$(git -C "$work/repo" rev-parse "$target")
tree=$(git -C "$work/repo" rev-parse "$target:archify")

# Extract without .gitattributes processing (git archive would apply
# export-ignore and export-subst), then prove the files are the commit's tree.
A="$work/src/archify"
mkdir -p "$A"
GIT_INDEX_FILE="$work/index" git -C "$work/repo" read-tree "$tree"
GIT_INDEX_FILE="$work/index" GIT_ATTR_NOSYSTEM=1 \
  git -C "$work/repo" -c core.autocrlf=false -c core.eol=lf checkout-index -a --prefix="$A/"

echo "commit $target"
echo "tree   $tree"
failed=0
fail() { echo "FAIL: $*"; failed=1; }

extracted_tree=$("$root/scripts/archify-tree-hash.sh" "$A")
[[ "$extracted_tree" == "$tree" ]] || fail "extracted files (tree $extracted_tree) differ from the commit tree"

# Lists files outside test/ that match a pattern, one per line.
hits() { (cd "$A" && grep -rlE "$1" --include='*.mjs' --include='*.js' --include='*.cjs' --include='*.html' . \
  | sed 's|^\./||' | grep -vE '^test/' | sort) || true; }

# Fails when a category has hits outside its allowlist.
only_in() {
  local name=$1 pattern=$2; shift 2
  local extra
  extra=$(hits "$pattern" | grep -vxF -f <(printf '%s\n' "$@" '') || true)
  if [[ -n "$extra" ]]; then fail "$name in files not in the reviewed list:"; echo "$extra" | sed 's/^/  /'; fi
}

echo "== .gitattributes and entry types"
if attrs=$(cd "$A" && grep -rnE "export-(ignore|subst)|filter=" --include=.gitattributes . 2>/dev/null); then
  fail ".gitattributes changes exported or checked-out content:"; echo "$attrs" | sed 's/^/  /'
fi
special=$(cd "$A" && find . ! -type f ! -type d | sort)
[[ -n "$special" ]] && { fail "symlinks or other non-regular files:"; echo "$special" | sed 's/^/  /'; }

echo "== package.json: install hooks and dependencies"
if [[ ! -f "$A/package.json" ]]; then
  fail "package.json is missing"
elif grep -nE '"(pre|post)?install"|"prepare"|"prepack"|"prepublish"|"(optional|peer|bundled|bundle)?[dD]ependencies"' "$A/package.json"; then
  fail "package.json has install hooks or dependencies other than devDependencies"
fi
[[ -d "$A/node_modules" ]] && fail "node_modules is present"

echo "== file types"
other=$(cd "$A" && find . -type f ! -name '*.mjs' ! -name '*.json' ! -name '*.md' ! -name '*.html' ! -name LICENSE | sort)
[[ -n "$other" ]] && { fail "unexpected file types:"; echo "$other" | sed 's/^/  /'; }

echo "== network access"
only_in "network access" \
  "node:(http|https|net|tls|dgram|dns)|['\"](http|https|net|tls|dgram|dns|undici)['\"]|fetch|WebSocket|XMLHttpRequest|sendBeacon|EventSource" \
  bin/preview.mjs renderers/shared/brand-marks.mjs scripts/check-update.mjs \
  renderers/shared/repository-evidence.mjs

echo "== process spawning"
only_in "process spawning" "child_process" \
  bin/archify.mjs bin/open-artifact.mjs bin/preview.mjs bin/visual-check.mjs \
  renderers/shared/repository-evidence.mjs scripts/render-examples.mjs

echo "== dynamic code"
only_in "dynamic code" "\beval\(|new Function|node:vm|\bimport\([^'\"]|require\([^'\"]" \
  bin/archify.mjs scripts/generate-validators.mjs

echo "== imports of packages (resolved from node_modules outside the tree)"
# import/require statements whose specifier is not relative and not node:.
pkg_re="(from[[:space:]]*|^[[:space:]]*import[[:space:]]*|import[[:space:]]*\\([[:space:]]*|require[[:space:]]*\\([[:space:]]*)['\"][^./'\"]"
pkg_files=$(cd "$A" && grep -rnE "$pkg_re" --include='*.mjs' --include='*.js' --include='*.cjs' . \
  | grep -vE "['\"]node:" | cut -d: -f1 | sed 's|^\./||' | grep -vE '^test/' | sort -u || true)
extra=$(echo "$pkg_files" | grep -vxF -e scripts/generate-validators.mjs -e scripts/generate-brand-marks.mjs -e '' || true)
[[ -n "$extra" ]] && { fail "package imports in files not in the reviewed list:"; echo "$extra" | sed 's/^/  /'; }

echo "== runtime files importing from test/"
only_in "import from test/" "(from|import|require)[[:space:]]*\(?[[:space:]]*['\"][^'\"]*test/"

echo "== secrets and persistence"
only_in "secret or persistence path" \
  "[/~]\.(ssh|aws|gnupg|netrc|bashrc|zshrc|profile|bash_profile)\b|git-credential|keychain|crontab|\.git/hooks|LaunchAgents|systemd"

echo "== opaque blobs (200+ base64 or hex characters)"
blobs=$(cd "$A" && grep -rlE "[A-Za-z0-9+/=]{200,}|[0-9a-fA-F]{200,}" --include='*.mjs' --include='*.js' --include='*.html' . | sed 's|^\./||' | grep -vE '^(test|examples)/' || true)
[[ -n "$blobs" ]] && { fail "opaque blobs:"; echo "$blobs" | sed 's/^/  /'; }

echo "== external hosts in the page template"
[[ -f "$A/assets/template.html" ]] || fail "assets/template.html is missing"
hosts=$(grep -ohE "https?://[a-zA-Z0-9.-]+" "$A/assets/template.html" 2>/dev/null | sort -u || true)
echo "$hosts" | sed 's/^/  /'
extra=$(echo "$hosts" | grep -vxE 'https://fonts\.googleapis\.com|https://fonts\.gstatic\.com|http://www\.w3\.org' || true)
[[ -n "$extra" ]] && fail "template loads new hosts: $extra"

echo "== URLs and commands in the agent instructions (read these)"
(cd "$A" && grep -rnoE "https?://[^ )\"'>]+|npx [^ ]+|curl [^ ]+|wget [^ ]+" --include='*.md' SKILL.md references | sort -u | sed 's/^/  /') || true

if [[ "$target" != "$pinned" ]]; then
  diff_file=$(mktemp "${TMPDIR:-/tmp}/archify-${pinned:0:7}-${target:0:7}-XXXXXX")
  git -C "$work/repo" diff "$pinned" "$target" -- archify > "$diff_file"
  echo "== changes since the pinned commit ${pinned:0:7}"
  git -C "$work/repo" diff --stat "$pinned" "$target" -- archify | tail -1
  echo "Read the full diff before updating $LOCK: $diff_file"
else
  [[ "$tree" == "$pinned_tree" ]] || fail "tree $tree does not match archify.lock ($pinned_tree)"
  echo "== installed copy: $installed"
  if [[ -d "$installed" ]]; then
    installed_tree=$("$root/scripts/archify-tree-hash.sh" "$installed")
    if [[ "$installed_tree" == "$pinned_tree" ]]; then
      echo "  identical to the pinned commit (tree $installed_tree)"
    else
      fail "installed copy (tree $installed_tree) differs from the pinned commit ($pinned_tree)"
      diff -rq "$A" "$installed" | sed 's/^/  /' || true
    fi
  else
    echo "  not installed"
  fi
fi

if [[ $failed -eq 0 ]]; then echo "All automated checks passed."; fi
exit $failed
