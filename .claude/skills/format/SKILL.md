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
   hawkeye format --config licenserc.toml
   ```

## Notes

- Run formatting commands (steps 1-4) first as they may auto-fix issues
- Clippy warnings should be treated as errors (`-D warnings`)
- If `taplo`, `zepter`, or `hawkeye` are not installed, inform the user how to install them:
  - `cargo install taplo-cli`
  - `cargo install zepter`
  - `cargo install hawkeye`
- If nightly fmt is not installed help user install with `rustup component add rustfmt --toolchain nightly`
- Report all errors found and fix them where possible
