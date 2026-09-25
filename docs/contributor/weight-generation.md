# Weight Generation

`/cmd bench` regenerates the weight files of the runtimes in `scripts/runtimes-matrix.json`.
For how to trigger commands and the common flags, see [Running Commands in PRs](commands-readme.md).

## What it does

For each selected runtime, `scripts/cmd/cmd.py`:

1. Builds the runtime with `--features runtime-benchmarks` and the `production` profile.
2. Lists the benchmarked pallets with `frame-omni-bencher`.
3. Runs `frame-omni-bencher v1 benchmark pallet` for each selected pallet (50 steps, 20 repeats by default).
4. Writes the result to `<runtime path>/src/weights/<pallet>.rs`.
   The `pallet_xcm_benchmarks::*` pallets go to `src/weights/xcm/` and use `templates/xcm-bench-template.hbs`.

The bot then commits the new weight files to the PR branch.
It also runs `subweight` against `origin/dev` and posts the weight changes in the result comment.

`frame-omni-bencher` comes from the Polkadot SDK release in `.github/env` (`POLKADOT_SDK_VERSION`).
The job runs on the `parity-weights` self-hosted runner, which matches the
[reference validator hardware](https://docs.polkadot.com/infrastructure/running-a-validator/requirements/#minimum-hardware-requirements).
`fmt` runs on `ubuntu-latest`.

`/cmd bench` does not update the `weights.rs` files inside the pallet crates (`crates/pallets/*/src/weights.rs`).
Those files use `templates/frame-weight-template.hbs` and you regenerate them by hand with `frame-omni-bencher`.

## Runtimes

| `--runtime` value | Crate | Weights directory |
|---|---|---|
| `web3-storage-paseo` | `storage-paseo-runtime` | `runtimes/web3-storage-paseo/src/weights` |
| `storage-parachain-runtime` | `storage-parachain-runtime` | `runtimes/web3-storage-local/src/weights` |

Without `--runtime`, the command runs for both.

## Examples

Regenerate every pallet in both runtimes (slow; only do this when necessary):

```sh
/cmd bench
```

Regenerate every pallet in one runtime:

```sh
/cmd bench --runtime web3-storage-paseo
```

Regenerate specific pallets in every runtime that contains all of them:

```sh
/cmd bench --pallet pallet_storage_provider pallet_drive_registry pallet_s3_registry
```

Regenerate specific pallets in one runtime:

```sh
/cmd bench --runtime web3-storage-paseo --pallet pallet_xcm_benchmarks::generic pallet_xcm_benchmarks::fungible
```

## Flags

`bench` accepts the common flags (`--quiet`, `--clean`, `--continue-on-fail`) and:

| Flag | Default | Meaning |
|---|---|---|
| `--runtime` | all runtimes | Runtime names from the table above, space separated. |
| `--pallet` | all pallets | Pallet names, space separated. A runtime is skipped unless it contains every listed pallet. |
| `--steps` | `50` | Samples across the variable components. |
| `--repeat` | `20` | Repetitions of each benchmark. |
| `--profile` | `production` | Cargo profile for the runtime build. |

By default the command stops at the first pallet that fails to benchmark.
Add `--continue-on-fail` to benchmark the remaining pallets and commit the successful ones.

## Notes

- The bot adds a 👀 reaction to your comment as soon as it receives it.
- GitHub queues the benchmark job until a `parity-weights` runner is free.
  Unless you passed `--quiet`, the bot posts a link to the pipeline when the job starts.
- The bot does not commit `Cargo.lock` changes.
