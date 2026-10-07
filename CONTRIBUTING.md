# Contributing to Scalable Web3 Storage

Thank you for your interest in contributing! This document covers the process
for submitting changes and the standards we follow.

## Getting Started

1. Make sure you meet the
   [Polkadot SDK requirements](https://paritytech.github.io/polkadot-sdk/master/polkadot_sdk_docs/reference_docs/development_environment_advice/index.html)
   (Rust nightly toolchain, `wasm32-unknown-unknown` target, etc.).
2. Fork the repository and clone your fork.
3. Run the one-time setup:

   ```bash
   just setup
   ```

4. Build and run tests:

   ```bash
   cargo build --release
   cargo test
   ```

## Rules

- All changes must go through a pull request — no direct pushes to `main`.
- Never force-push (`git push --force`).
- CI must pass before merging.
- Address all review comments or explain why you disagree.

## Pull Request Process

1. Branch from `main` (or the current development branch).
2. Keep PRs focused on a single concern.
3. Open as **Draft** while work is in progress; mark **Ready for review** when
   done.
4. Write a clear description: what changed, why, and how to test.
5. Ensure `cargo test`, `cargo clippy`, and formatting all pass locally before
   requesting review.

## Code Style

```bash
# Rust formatting (requires nightly)
cargo +nightly fmt --all

# TOML formatting
taplo format --check --config .config/taplo.toml

# Clippy linting
cargo clippy --all-targets --all-features --workspace -- -D warnings
```

See [CLAUDE.md](./CLAUDE.md) for additional code review guidelines and project
conventions.

## PR Bot Commands

Comment `/cmd <command>` on a pull request to run it against the PR branch.
The bot commits any resulting changes back to the branch and posts the result
as a PR comment. Only members of the `paritytech` GitHub organization can run
commands. `/cmd --help` and `/cmd <command> --help` list the full flags.

- `/cmd fmt` runs `cargo +nightly fmt` and
  `taplo format --config .config/taplo.toml`, then commits the result.
- `/cmd bench` regenerates the runtime weight files
  (`<runtime path>/src/weights/`) with `frame-omni-bencher`, for the runtimes
  in `scripts/runtimes-matrix.json` (`web3-storage-paseo`,
  `storage-parachain-runtime`). It does not touch the pallet crates'
  `crates/pallets/*/src/weights.rs` — regenerate those by hand with
  `frame-omni-bencher`.

Common flags for every command:

| Flag | Meaning |
|---|---|
| `--quiet` | Don't post the start/end PR comments (reactions still show). Check the [Actions tab](https://github.com/paritytech/web3-storage/actions/workflows/cmd.yml) for status. |
| `--clean` | Delete earlier `/cmd` comments and bot replies on the PR before running. |
| `--continue-on-fail` | Don't stop at the first failure; commit/report the ones that succeeded. |

`bench`-only flags:

| Flag | Default | Meaning |
|---|---|---|
| `--runtime` | all runtimes | `web3-storage-paseo` and/or `storage-parachain-runtime`, space separated. |
| `--pallet` | all pallets | Pallet names, space separated. A runtime is skipped unless it contains every listed pallet. |
| `--steps` | `50` | Samples across the variable components. |
| `--repeat` | `20` | Repetitions of each benchmark. |
| `--profile` | `production` | Cargo profile for the runtime build. |

Examples:

```sh
/cmd fmt --quiet
/cmd bench --runtime web3-storage-paseo --pallet pallet_storage_provider
```

## Licensing

This repository is dual-licensed. The license that applies depends on the crate:

| License | Crates |
|---------|--------|
| [GPL-3.0-only](LICENSE-GPL3) | `runtimes/`, `provider-node/`, `user-interfaces/` |
| [Apache-2.0](LICENSE-APACHE2) | `crates/pallets/` (pallets and their precompiles), `crates/primitives/`, `clients/` |

Each crate declares its applicable license in its `Cargo.toml` or
`package.json`.

**By submitting a pull request, you agree to license your contribution under
the same license that applies to the crate(s) you are modifying.**

## Security

The security policy for this project is governed by the
[paritytech organization-level security policy](https://github.com/paritytech/.github/blob/master/SECURITY.md).
If you discover a vulnerability, please follow the responsible disclosure
process described there — do **not** open a public issue.
