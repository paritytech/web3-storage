# storage-cli

Operator CLI for [scalable Web3 storage](../../../README.md). It consolidates the
on-chain and off-chain storage operations that were previously scattered across
`clients/storage/examples/*` and ad-hoc scripts into a single ergonomic binary,
built on top of the [`storage-client`](../../../clients/storage) SDK.

## Usage

```bash
cargo run -p storage-cli -- --help
cargo run -p storage-cli -- provider register --help
cargo run -p storage-cli -- stress-test upload --help
```

### Global flags

| Flag                   | Default                 | Env            | Description                                        |
| ---------------------- | ----------------------- | -------------- | -------------------------------------------------- |
| `--chain-rpc <URL>`    | `ws://127.0.0.1:2222`   | `CHAIN_RPC`    | Parachain RPC WebSocket endpoint.                  |
| `--provider-url <URL>` | `http://127.0.0.1:3333` | `PROVIDER_URL` | Provider node HTTP endpoint.                       |
| `--output <FORMAT>`    | `text`                  |                | Metrics summary format: `text` or `json`.          |

Each command takes the secret of the account it signs as, named after the
account's role: `--provider-suri`/`--provider-keyfile` or
`--client-suri`/`--client-keyfile`. Exactly one of each pair is required. A
keyfile contains the SURI.

## `provider register`

Registers the provider account as a storage provider, locking the
1000-token minimum stake, and sets it to accept primary agreements at 1 per
byte-block for 10 to 100,000 anchor blocks. If the account is already
registered with the same signing key, it does nothing; with a different key, it
exits with an error.

```bash
cargo run -p storage-cli -- \
  provider register \
  --provider-suri //Alice \
  --multiaddr /ip4/127.0.0.1/tcp/3333
```

| Param                    | Default   | Description                                                           |
| ------------------------ | --------- | --------------------------------------------------------------------- |
| `--provider-suri <SURI>` | —         | Secret URI of the provider account, e.g. `//Alice`.                   |
| `--provider-keyfile <FILE>` | —      | File with the provider account's SURI: the file the node's `--keyfile` reads. |
| `--multiaddr <ADDR>`     | required  | Multiaddr at which clients reach the provider node.                   |
| `--key-scheme <SCHEME>`  | `sr25519` | Signing key to register: `sr25519`, `ed25519`, `ecdsa` or `eth`. Must match the node's `--key-scheme`. |

The extrinsics are always signed by the sr25519 account. The provider node can
start before or after registration. If it starts after, it reads the
registration when it connects and can sign terms on its first request.

## `stress-test upload`

Drives configurable upload load against a provider: `--users` simulated clients
each performing `--uploads-per-user` uploads, with either axis run sequentially
or in parallel. Targets buckets the client account already has a storage agreement with
the given provider for, or buckets the run prepares itself with
`--prepare-buckets`.

```bash
cargo run -p storage-cli -- \
  stress-test upload \
  --client-suri //Bob \
  --provider-account <PROVIDER_SS58> \
  --prepare-buckets 2 \
  --users 10 \
  --uploads-per-user 5 \
  --payload-size 1048576 \
  --parallel-uploads
```

| Param                        | Default            | Description                                                |
| ---------------------------- | ------------------ | ---------------------------------------------------------- |
| `--client-suri <SURI>`       | —                  | Secret URI of the client account, e.g. `//Bob`.            |
| `--client-keyfile <FILE>`    | —                  | File with the client account's SURI.                       |
| `--provider-account <ACCOUNT>` | required         | Public account of the provider (SS58 or `0x`-hex) whose agreements select the target buckets. |
| `--max-buckets-to-write <N>` | all buckets        | Cap the number of buckets written to (must be `>= 1`).     |
| `--users <N>`                | `1`                | Number of simulated users, each with its own client.       |
| `--uploads-per-user <X>`     | `1`                | Number of uploads each user performs.                      |
| `--payload-size <BYTES>`     | `524288` (0.5 MiB) | Exact size of each randomly-generated payload.             |
| `--parallel-users`           | off (sequential)   | Run users in parallel.                                     |
| `--parallel-uploads`         | off (sequential)   | Run each user's uploads in parallel.                       |
| `--max-concurrency <N>`      | `0` (unbounded)    | Cap total in-flight uploads across all users.              |
| `--prepare-buckets <N>`      | `0` (off)          | Open `N` fresh buckets with a primary agreement to `--provider-account` before the run. |
| `--prepare-max-bytes <BYTES>`| per-bucket load + 10% | Quota negotiated per prepared bucket. Rejected if below what the run writes to one bucket. |
| `--prepare-duration <BLOCKS>`| `100`              | Agreement duration for prepared buckets, in anchor blocks. |
| `--prepare-price-per-byte <P>`| `1`               | Price per byte per block the run accepts. The provider rejects lower offers and signs at its listed price. |
| `--bucket-source <MODE>`     | `create` if preparing, else `discover` | `discover`: existing buckets only. `create`: prepared buckets only. `auto`: existing first, prepare the shortfall up to `--prepare-buckets`. |

Total uploads is `users × uploads-per-user`. The two `--parallel-*` flags are
independent axes: `--parallel-users` alone runs each user's uploads in sequence
but the users concurrently, `--parallel-uploads` alone does the reverse, and both
together maximise concurrency (bounded by `--max-concurrency` if set).

**Behavior**

1. Derives the client account from `--client-suri`/`--client-keyfile`. This
   account signs the `Authorization` header on every provider request, so it
   must be a member of the buckets written to. Unless `--bucket-source create`,
   reads its buckets from chain (`MemberBuckets[account]`) and keeps those with a
   `StorageAgreements[bucket][provider]` entry for `--provider-account`.
2. With `--prepare-buckets N`, negotiates terms with the provider node
   (`POST /negotiate`) and redeems them on-chain (`create_bucket_with_primary`)
   once per bucket, the same path as the SDK's `complete_workflow` example.
   Preparation runs before the measurement clock starts and never appears in the
   throughput or latency numbers. A preparation failure exits non-zero with no
   metrics summary.
3. If the resulting target set is empty, it exits with an error.
4. Runs the configured load over the provider's HTTP API, assigning each upload a
   target bucket round-robin so writes spread evenly across the selected buckets.

`--max-buckets-to-write` is applied before preparation: in `create` mode it must
be at least `--prepare-buckets`, so no bucket is opened and then never written.

### Output

Progress goes to stderr. stdout carries the preparation report (text mode) and
the metrics summary, so `--output json` stays parseable when piped.

```
preparation: 2 bucket(s) created in 14.210s
  buckets:    #17, #18
  agreement:  28835840 bytes x 100 blocks at 1 per byte-block
upload results: 50 total, 50 ok, 0 failed
  data:       50.00 MiB uploaded in 0.050s
  throughput: 1003.46 MiB/s, 1003.5 uploads/s
  latency:    min 0.031s / avg 0.040s / max 0.048s
  percentile: p50 0.040s / p95 0.048s / p99 0.048s
```

`--output json` emits one object: `preparation` (`null` when nothing was
prepared; otherwise `buckets`, `max_bytes`, `duration`, `price_per_byte` as a
decimal string, `elapsed_secs`) and `results`, an array of summary objects with
the fields `operation`, `total`, `ok`, `failed`, `bytes_ok`, `elapsed_secs`,
`throughput_bytes_per_sec`, `throughput_ops_per_sec`,
`latency_{min,avg,max,p50,p95,p99}_secs` and `sample_errors`. The figures in
`results` cover the upload phase only.

Per-upload failures are folded into the metrics rather than aborting the run. The
command exits non-zero on setup failures (bad provider, chain unreachable, no
target buckets, preparation errors) and when every operation of a scenario
failed; a run with some failures still exits zero, so check `failed` in the
summary.

### Required on-chain setup

The provider must be registered on-chain and its node running (`just start-chain`,
`storage-cli provider register`, `just start-provider`).
Buckets and agreements either exist already (default `discover` mode) or the run
opens them with `--prepare-buckets N`. The client account pays for the prepared
agreements at the provider's listed price.

## `soak gc`

Drives the provider's garbage collector through repeated prune cycles and bucket teardowns until a deadline.
It is a manual correctness test, not part of `just e2e` or CI.
It attaches to a running chain and provider and starts no process.

Each lane is one client account with its own bucket, and lanes run in parallel.
An epoch is one bucket per lane: the lane runs `--cycles-per-bucket` prune cycles, then ends its agreement.
After each epoch the run waits until the provider holds no bucket, node or byte, which shows the garbage collector erased every retired bucket.

A cycle uploads `--chunks-per-cycle` unique chunks and checkpoints them.
It checks the provider's used bytes and quota, and checks that an upload over the remaining capacity is rejected with HTTP 507.
It then prunes every chunk but the newest, confirms the deletion and checkpoints the prune.
It waits until the provider reports only the surviving chunk as used, checks that the pruned chunks return 404, and checks that the surviving chunk downloads.

### Setup

Start the dev chain, then a provider with an empty storage path and a short garbage collector rescan interval.
Condemned buckets are erased on a later pass, so the default 10 minute interval makes every drain check fail.

```bash
just start-e2e-chain
GC_SCAN_INTERVAL=30 just PROVIDER_STORAGE_PATH=/tmp/soak-db start-provider 2>&1 | tee /tmp/provider.log
just register-provider
```

### Run

```bash
just gc-soak \
  --provider-account 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY \
  --client-suri //Bob --client-suri //Dave \
  --duration 30m \
  --provider-log /tmp/provider.log
```

| Param                          | Default   | Description                                                                 |
| ------------------------------ | --------- | --------------------------------------------------------------------------- |
| `--client-suri <SURI>`         | -         | Client account of one lane. Repeat for more lanes.                          |
| `--client-keyfile <FILE>`      | -         | File with a lane account's SURI. Repeat for more lanes. At least one account is required, and two lanes cannot share an account. |
| `--provider-account <ACCOUNT>` | required  | Provider account (SS58 or `0x`-hex) the buckets are opened with.            |
| `--cycles-per-bucket <N>`      | `5`       | Prune cycles before a bucket's agreement ends.                              |
| `--chunks-per-cycle <N>`       | `3`       | Chunks uploaded per cycle. At least 2.                                      |
| `--chunk-bytes <BYTES>`        | `100`     | Bytes per chunk, from 64 up to the chunk size limit.                        |
| `--duration <DURATION>`        | `30m`     | Stop starting epochs after this long (`90s`, `30m`, `4h`). A started epoch always finishes. Excludes `--epochs`. |
| `--epochs <N>`                 | -         | Stop after this many epochs.                                                |
| `--agreement-duration <BLOCKS>`| `600`     | Agreement length of each bucket, in anchor blocks.                          |
| `--gc-scan-interval <SECS>`    | `30`      | The provider's `GC_SCAN_INTERVAL`. A drain may take up to twice this plus 60 seconds. |
| `--erase-timeout <SECS>`       | `120`     | Longest wait for one erasure.                                               |
| `--provider-log <PATH>`        | -         | Provider log file. A "GC reconcile skipped" line fails the run, and `WARN` and `ERROR` lines of the garbage collector are counted. |

Each lane pays for one agreement per epoch at the provider's listed price.
The run prints the cost of one agreement at the start.
Preflight fails when the provider already holds data, is not registered, or is not accepting primary agreements.

The first Ctrl-C stops new cycles, ends the agreements, waits for the drain and prints the report.
A second Ctrl-C exits at once.
After a failure the run leaves its buckets as they are.

### Output

Progress goes to stderr.
stdout carries the report: one line per epoch with the erase latency (min, p50, p95, max), the drain time, the provider's memory and the number of garbage collector warnings.
The memory is read for a provider on this machine only, and is reported without a threshold.
`--output json` prints the same data as `cycles`, `barriers` and `failure` objects.
The command exits non-zero on the first broken invariant and names the epoch, lane and cycle.

## Limitations

- **Agreement expiry is not checked.** A bucket is selected based on the presence
  of a `StorageAgreements[bucket][provider]` entry; expired-but-not-yet-cleared
  agreements are treated as matches.
