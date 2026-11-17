---
name: format
description: Run all formatting, linting and cleaning checks before committing code
---

Run all formatting, linting, and cleaning tasks that should be done before committing code. Fix any issues found automatically where possible.

## Steps

1. **Rust formatting** (requires nightly):
   ```bash
   cargo +nightly fmt --all
   ```

2. **TOML formatting**:
   ```bash
   taplo format --config .config/taplo.toml
   ```

3. **Zepter checks** (feature propagation):
   ```bash
   zepter run --config .config/zepter.yaml
   ```

4. **Clippy linting**:
   ```bash
   cargo clippy --all-targets --all-features --workspace -- -D warnings
   ```

5. **License headers** (SPDX):
   ```bash
   hawkeye format --config licenserc.apache.toml
   hawkeye format --config licenserc.gpl.toml
   ```
   - `licenserc.apache.toml` covers library/SDK crates (pallets, primitives, client SDKs, precompiles, examples, `packages/*`) — Apache-2.0
   - `licenserc.gpl.toml` covers runtime/node crates (parachain runtimes, provider node, UI apps) — GPL-3.0-only

6. **Markdown links** (offline, relative links only; run when any `*.md` changed):
   ```bash
   lychee --config .config/lychee.toml --no-progress "./**/*.md"
   ```

7. **Docs site** (run when `docs/design/` changed):
   ```bash
   scripts/check-docs-site.sh
   ```
   - If it fails, update `docs/site/` with the `/generate-docs` skill, then run `scripts/check-docs-site.sh --update`
   - If `.claude/skills/generate-docs/archify.lock` changed, also run `scripts/verify-archify.sh`

8. **TypeScript typecheck** (run when `packages/`, `examples/papi/` or `user-interfaces/` changed):
   ```bash
   pnpm dedupe --check
   pnpm run typecheck:packages
   pnpm --filter <ui-package> run build
   ```
   - `build` runs `tsc -b` and then `vite build`, as the CI `Typecheck + build` job does
   - `<ui-package>` is each changed UI: `@web3-storage/drive-ui`, `@web3-storage/s3-ui`, `provider-dashboard`, `@web3-storage/photos`, `@web3-storage/explorer`
   - Run `pnpm install` first if dependencies changed

## Notes

- Run formatting commands (steps 1-4) first as they may auto-fix issues
- Clippy warnings should be treated as errors (`-D warnings`)
- If `taplo`, `zepter`, `hawkeye` or `lychee` are not installed, inform the user how to install them:
  - `cargo install taplo-cli`
  - `cargo install zepter`
  - `cargo install hawkeye`
  - `brew install lychee` or `cargo install lychee`
- If nightly fmt is not installed help user install with `rustup component add rustfmt --toolchain nightly`
- Report all errors found and fix them where possible
