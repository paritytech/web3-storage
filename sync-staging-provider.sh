#!/usr/bin/env bash
# Sync the provider node and its crates from origin/dev into staging-provider.
#
# staging-provider = staging + provider node. Everything staging owns (pallet,
# primitives, runtime, docs) comes from merging staging, never from here.
# By default the provider code is taken from the dev commit staging is synced
# to (staging's newest `Synced-From` trailer), so it builds against the same
# pallet and primitives; pass a dev revision to override.
#
# Staging-provider-owned files (never synced): Cargo.toml (members and
# provider workspace dependencies), .github/workflows/check.yml (branch
# filters), sync-staging-provider.sh.
set -euo pipefail

# Copied from dev as-is.
SYNC_PATHS=(
  provider-node
  crates/providers
  crates/storage-subxt
  # Metadata snapshot that provider-node's chain_follower tests include_bytes!.
  packages/papi/.papi/metadata/parachain.scale
)

TRAILER=Provider-Synced-From

main() {
  cd "$(git rev-parse --show-toplevel)"
  [[ $(git branch --show-current) == staging-provider ]] || { echo "not on staging-provider"; exit 1; }
  git diff --quiet && git diff --cached --quiet || { echo "working tree not clean"; exit 1; }

  git fetch origin
  if git rev-parse -q --verify origin/staging-provider >/dev/null; then
    git merge --ff-only origin/staging-provider
  fi
  git merge-base --is-ancestor origin/staging HEAD \
    || { echo "origin/staging moved: 'git merge origin/staging' first (resolve Cargo.lock by re-running cargo check)"; exit 1; }

  local new prev
  if [[ $# -gt 0 ]]; then
    new=$(git rev-parse "$1^{commit}")
  else
    new=$(git log -1 origin/staging --grep='^Synced-From:' --format='%(trailers:key=Synced-From,valueonly)' | tr -d '[:space:]')
    [[ -n $new ]] || { echo "no Synced-From trailer on origin/staging"; exit 1; }
  fi
  prev=$(git log -1 --grep="^$TRAILER:" --format="%(trailers:key=$TRAILER,valueonly)" | tr -d '[:space:]')
  [[ -n $prev ]] || { echo "no $TRAILER trailer found"; exit 1; }
  [[ $prev != "$new" ]] || { echo "already at dev @ ${new:0:10}"; exit 0; }

  # Remove first so files deleted on dev disappear here too.
  git rm -r -q --ignore-unmatch -- "${SYNC_PATHS[@]}"
  git restore --source="$new" --staged --worktree -- "${SYNC_PATHS[@]}"
  if git diff --cached --quiet; then echo "no provider changes up to dev @ ${new:0:10}"; exit 0; fi

  echo "== staging-provider-owned files changed on dev since last sync (port by hand if relevant) =="
  git --no-pager diff "$prev" "$new" -- Cargo.toml

  # Updates Cargo.lock for the synced crates. Override CARGO_CHECK to build
  # elsewhere, e.g. through cargo remote (it copies Cargo.lock back).
  ${CARGO_CHECK:-cargo check --workspace --all-targets}
  git add -A -- "${SYNC_PATHS[@]}" Cargo.lock

  {
    echo "staging-provider: sync from dev @ ${new:0:10}"; echo
    git log --oneline --no-merges "$prev..$new" -- "${SYNC_PATHS[@]}"; echo
    echo "$TRAILER: $new"
  } | git commit -F -

  echo "Review with 'git show --stat', port any Cargo.toml changes (git commit --amend), then 'git push'."
}

main "$@"; exit
