#!/usr/bin/env bash
# Sync in-scope runtime code and design docs from origin/dev into staging.
# Staging-owned files (never synced): Cargo.toml, .github/workflows/check.yml,
# sync-staging.sh.
set -euo pipefail

# Copied from dev as-is.
SYNC_PATHS=(
  crates/pallets/storage-provider   # minus EXCLUDE_PATHS below
  crates/primitives/storage
  .gitignore
  .github/env
  .github/workflows/set-image.yml
  .config/taplo.toml
  .config/zepter.yaml
  LICENSE-APACHE2
  LICENSE-GPL3
  licenserc.apache.toml
  licenserc.gpl.toml
  docs/design
)

# Subdirectories of SYNC_PATHS that are out of scope.
EXCLUDE_PATHS=(
  crates/pallets/storage-provider/precompiles
)

# Modified on staging (drive, S3, file-system and precompile crates removed),
# so dev's changes since the last sync are applied as a 3-way patch instead.
PATCH_PATHS=(
  runtimes/web3-storage-paseo
)

main() {
  cd "$(git rev-parse --show-toplevel)"
  [[ $(git branch --show-current) == staging ]] || { echo "not on staging"; exit 1; }
  git diff --quiet && git diff --cached --quiet || { echo "working tree not clean"; exit 1; }

  git fetch origin
  git merge --ff-only origin/staging

  local new prev
  new=$(git rev-parse origin/dev)
  prev=$(git log -1 --grep='^Synced-From:' --format='%(trailers:key=Synced-From,valueonly)' | tr -d '[:space:]')
  [[ -n $prev ]] || { echo "no Synced-From trailer found"; exit 1; }

  git restore --source="$new" --staged --worktree -- "${SYNC_PATHS[@]}" Cargo.lock rust-toolchain.toml
  git rm -r -q --cached --ignore-unmatch -- "${EXCLUDE_PATHS[@]}"
  rm -rf -- "${EXCLUDE_PATHS[@]}"
  if ! git diff --quiet "$prev" "$new" -- "${PATCH_PATHS[@]}"; then
    git diff "$prev" "$new" -- "${PATCH_PATHS[@]}" | git apply --3way \
      || { echo "conflicts in ${PATCH_PATHS[*]}: resolve, then 'git add' and commit with a 'Synced-From: $new' trailer"; exit 1; }
  fi
  if git diff --cached --quiet; then echo "already at dev @ ${new:0:10}"; exit 0; fi

  echo "== staging-owned files changed on dev since last sync (port by hand if relevant) =="
  git --no-pager diff "$prev" "$new" -- Cargo.toml .github/workflows/check.yml

  cargo check --workspace
  git add -A -- "${SYNC_PATHS[@]}" "${PATCH_PATHS[@]}" Cargo.lock rust-toolchain.toml

  {
    echo "staging: sync from dev @ ${new:0:10}"; echo
    git log --oneline --no-merges "$prev..$new" -- "${SYNC_PATHS[@]}" "${PATCH_PATHS[@]}"; echo
    echo "Synced-From: $new"
  } | git commit -F -

  echo "Review with 'git show --stat', port any Cargo.toml changes (git commit --amend), then 'git push'."
}

main "$@"; exit
