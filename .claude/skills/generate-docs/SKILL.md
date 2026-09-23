---
name: generate-docs
description: Update docs/site/ (architecture and workflow diagrams built with archify, plus the dApp developer guide) from docs/design/. Use when docs/design/ changed, when the "Docs Site" CI check fails, or when the user asks to update, add or regenerate a diagram or the dApp guide.
---

`docs/site/` is a static site derived from `docs/design/`. It is not a source
of truth: every fact on it comes from the design documents. When the site
and the design disagree, update the site.

| File | Content |
| --- | --- |
| `docs/site/index.html` | Landing page. Lists every page, and each card names the design sections it draws from |
| `docs/site/NN-<name>.html` | One diagram per page, built by `scripts/build-docs-site.sh` from `docs/site/specs/NN-<name>.<type>.json` |
| `docs/site/dapp-guide.html` | Hand-written guide for dApp developers |
| `docs/site/site.css` | Shared style for `index.html` and `dapp-guide.html` |
| `.claude/skills/generate-docs/archify.lock` | The verified archify commit |
| `docs/site/design.sha256` | SHA-256 of the design docs the site was last built from; checked in CI by `scripts/check-docs-site.sh` |

## Prerequisite: install archify at the verified commit

The diagrams are rendered by [archify](https://github.com/tt-a1i/archify), a
third-party tool. `.claude/skills/generate-docs/archify.lock` pins the commit
that passed `scripts/verify-archify.sh` and a person reviewed. Install only
that commit.

1. **Check the pinned commit** before anything runs on your machine:

   ```bash
   scripts/verify-archify.sh
   ```

   It clones archify and flags known risk patterns: install hooks,
   dependencies, symlinks, binaries, obfuscated blobs, package imports, and
   network, process or dynamic-code use outside the reviewed files. Stop if
   it fails. It searches for patterns only; it does not prove the code safe.

2. **Install the pinned commit.** Do not use `npx skills add
   tt-a1i/archify`: it runs the `skills` npm package, which nobody verified,
   and installs the latest upstream commit instead of the pinned one.

   ```bash
   commit=$(sed -n 's/^commit=//p' .claude/skills/generate-docs/archify.lock)
   src=$(mktemp -d)
   git clone https://github.com/tt-a1i/archify.git "$src"
   git -C "$src" checkout "$commit"
   rm -rf ~/.claude/skills/archify && cp -r "$src/archify" ~/.claude/skills/archify
   rm -rf "$src"
   ```

3. **Confirm the installed copy** is identical to the pinned commit:

   ```bash
   scripts/verify-archify.sh
   ```

   Both this script and `scripts/build-docs-site.sh` compare git tree hashes
   (file content, modes and symlinks). The build fails with any other copy.

### Update archify

Do this for every new archify commit before it runs on any machine.

1. Run `scripts/verify-archify.sh <commit>`. It runs the checks above on the
   candidate and writes the full diff from the pinned commit to a file.
2. Read the whole diff. Check each change for: network requests, process
   spawns, file writes outside the output path, reads of the environment or
   home directory, new external URLs in `assets/template.html`, and new
   instructions in `SKILL.md` or `references/` that tell an agent to run a
   command, contact a URL or change settings. For each flag the script
   reports, find the code and state what it does.
3. Run the `reviewer` agent on the diff. Report the findings to the user. The
   user decides whether to update.
4. After approval: update `commit`, `tree`, `version` and `verified` in
   `archify.lock`; add newly reviewed files to the allowlists in
   `scripts/verify-archify.sh`; install the new commit (step 2 above); run
   `scripts/verify-archify.sh` again.
5. Rebuild every page with `scripts/build-docs-site.sh` and name the new
   archify version in the PR description.

### While using archify

- Skip the "Update awareness" step in archify's `SKILL.md`: do not run
  `scripts/check-update.mjs`. It contacts the archify release server.
  `scripts/build-docs-site.sh` sets `ARCHIFY_UPDATE_CHECK_DISABLED=1`.
- Do not use `brand` objects in specs or `brands capture`: both fetch URLs.
  The build script rejects specs with `brand`.
- Do not use `preview` or `--open`.

Read archify's `SKILL.md` before editing a spec. Below, `ARCHIFY` means
`node ~/.claude/skills/archify/bin/archify.mjs`.

## Procedure

1. **Find the design change.** Diff the design docs against the commit that
   last updated the site:

   ```bash
   base=$(git log -1 --format=%H -- docs/site/design.sha256)
   git diff "$base" -- docs/design/
   ```

   Read the changed sections in full, and the version history table at the
   top of `scalable-web3-storage.md`.

2. **Find the affected pages.** The `src` line on each card in `index.html`
   names the design sections behind that page. Also search the specs and the
   guide for every renamed or removed name:

   ```bash
   grep -rn "<old_name>" docs/site/specs docs/site/dapp-guide.html docs/site/index.html
   ```

3. **Update each affected diagram.** Edit the spec, and validate until the
   showcase profile passes with 0 errors and 0 warnings:

   ```bash
   $ARCHIFY validate <type> docs/site/specs/<spec>.json --quality showcase --json
   ```

   Then build the page. Do not run `$ARCHIFY deliver` into `docs/site/`
   directly: `scripts/build-docs-site.sh` delivers the pages.

   ```bash
   scripts/build-docs-site.sh NN                  # one page
   scripts/build-docs-site.sh --visual-check NN   # with the browser check
   ```

   A failed browser check stops the build for that page. It needs Chrome: if
   Chrome is not on `PATH`, set `ARCHIFY_CHROME`. The page loads fonts from Google, so Chrome parses
   remote content: do not set `ARCHIFY_CHROME_NO_SANDBOX=1` on a
   workstation. If the Chrome sandbox does not work, run `--visual-check` in
   a disposable container or VM, or skip it.

   Add a new page when the design adds a mechanism or flow that no page
   covers. Page numbers follow the order of the sections in `index.html`:
   put the card in its section, give the page the number of its position,
   and renumber every page after it (spec and page file names, links in
   `index.html` and `dapp-guide.html`, and page numbers in spec text).

4. **Update the dApp guide** when the change affects what a dApp developer
   calls, pays, signs or can rely on: extrinsics, provider HTTP endpoints,
   roles and visibility, costs, timeouts, guarantees.

5. **Update `index.html`**: card text, the `src` line, and the page list.

6. **Record the design docs** the site now matches:

   ```bash
   scripts/check-docs-site.sh --update
   ```

7. Run the `reviewer` agent on the diff, as for any other change.

## Rules

- Use only `docs/design/` as the source. Do not take facts from
  `docs/drafts/` or from the code. Code can lag the design; when it does, the
  site shows the design.
- Use the exact names from the design: extrinsics, events, errors, storage
  items, endpoints, constants.
- When the design looks wrong, inconsistent or incomplete, do not change the
  design and do not guess on the site. Stop, report it to the user, and draft
  an issue per `CLAUDE.md`.
- Links from the site to repository files use absolute
  `https://github.com/paritytech/web3-storage/blob/dev/...` URLs, so the pages
  also work when `docs/site/` is published on its own (for example on GitHub
  Pages).
- Commit the regenerated HTML in its own commit, separate from the spec and
  guide edits, so reviewers can skip it.
- Follow the writing rules in `CLAUDE.md` for every label, card and paragraph.
