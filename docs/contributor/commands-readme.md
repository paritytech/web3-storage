# Running Commands in PRs

Comment `/cmd <command>` on a pull request to run a command against the PR branch.
The bot commits any resulting changes back to the PR branch and posts the result as a PR comment.
Only members of the `paritytech` GitHub organization can run commands; for other users the job does not run.

## Usage

- `/cmd --help` lists all commands and their flags.
- `/cmd <command> --help` shows the flags of one command.

### Commands

- `/cmd fmt` runs `cargo +nightly fmt` and `taplo format --config .config/taplo.toml`, then commits the result.
- `/cmd bench` regenerates runtime weights with `frame-omni-bencher`.
  See [Weight Generation](weight-generation.md).

### Flags

These flags work with every command:

- `--quiet`: do not post the start and end comments.
  The bot still adds reactions to your comment.
  The pipeline status is on the [Actions tab](https://github.com/paritytech/web3-storage/actions/workflows/cmd.yml).
- `--clean`: delete your earlier `/cmd` comments and the bot's replies on the PR before running.
  Use it after several reruns to keep the PR readable.
- `--continue-on-fail`: do not stop at the first failure.
  For `bench`, the bot commits the weights of the pallets that succeeded and lists the ones that failed.

### Where the command code comes from

- `.github/workflows/cmd.yml` reacts to the comment.
  GitHub runs it from the default branch (`dev`).
- `.github/workflows/cmd-run.yml` and `scripts/cmd/cmd.py` run from the `cmd-bot` branch.
- The command itself runs on a checkout of the PR branch, so it uses the PR's `scripts/runtimes-matrix.json` and code.

A change to `cmd.yml` takes effect after it merges into `dev`.
A change to `cmd-run.yml` or `scripts/cmd/` takes effect after it lands on `cmd-bot`.
To test a new command before that, run it in a fork where you control those branches.

### Examples

`cmd.yml` accepts a comment only if it matches `^(\/cmd )([-\/\s\w.=:]+)$`.
Arguments may contain letters, digits, `_`, whitespace, `-`, `/`, `.`, `=` and `:`.

```sh
/cmd bench --runtime web3-storage-paseo --pallet=pallet_storage_provider
/cmd bench --pallet pallet_storage_provider pallet_drive_registry
/cmd fmt --quiet
```
