# storage-cli

Operator CLI for [scalable Web3 storage](../../../README.md). It consolidates the
on-chain and off-chain storage operations that were previously scattered across
`clients/storage/examples/*` and ad-hoc scripts into a single ergonomic binary,
built on top of the [`storage-client`](../../../clients/storage) SDK.

## Usage

```bash
cargo run -p storage-cli -- --help
cargo run -p storage-cli -- stress-test upload --help
```

### Global flags

| Flag                   | Default                 | Env            | Description                                        |
| ---------------------- | ----------------------- | -------------- | -------------------------------------------------- |
| `--chain-rpc <URL>`    | `ws://127.0.0.1:2222`   | `CHAIN_RPC`    | Parachain RPC WebSocket endpoint.                  |
| `--provider-url <URL>` | `http://127.0.0.1:3333` | `PROVIDER_URL` | Provider node HTTP endpoint.                       |
| `--suri <SURI>`        | —                       |                | Secret URI for the account, e.g. `//Alice`.        |
| `--keyfile <FILE>`     | —                       |                | File whose contents are the SURI/seed.             |
| `--output <FORMAT>`    | `text`                  |                | Metrics summary format: `text` or `json`.          |

`--suri` and `--keyfile` are mutually exclusive; exactly one is required. The
account they resolve to signs the `Authorization` header on every provider
request, so it must be a member of the buckets being written to.

## `stress-test upload`

Drives configurable upload load against a provider: `--users` simulated clients
each performing `--uploads-per-user` uploads, with either axis run sequentially
or in parallel. Targets buckets the account already has a storage agreement with
the given provider for, or buckets the run prepares itself with
`--prepare-buckets`.

```bash
cargo run -p storage-cli -- \
  --suri //Bob \
  stress-test upload \
  --provider <PROVIDER_SS58> \
  --prepare-buckets 2 \
  --users 10 \
  --uploads-per-user 5 \
  --payload-size 1048576 \
  --parallel-uploads
```

| Param                        | Default            | Description                                                |
| ---------------------------- | ------------------ | ---------------------------------------------------------- |
| `--provider <ACCOUNT>`       | required           | Provider account (SS58 or `0x`-hex) whose agreements select the target buckets. |
| `--max-buckets-to-write <N>` | all buckets        | Cap the number of buckets written to (must be `>= 1`).     |
| `--users <N>`                | `1`                | Number of simulated users, each with its own client.       |
| `--uploads-per-user <X>`     | `1`                | Number of uploads each user performs.                      |
| `--payload-size <BYTES>`     | `524288` (0.5 MiB) | Exact size of each randomly-generated payload.             |
| `--parallel-users`           | off (sequential)   | Run users in parallel.                                     |
| `--parallel-uploads`         | off (sequential)   | Run each user's uploads in parallel.                       |
| `--max-concurrency <N>`      | `0` (unbounded)    | Cap total in-flight uploads across all users.              |
| `--prepare-buckets <N>`      | `0` (off)          | Open `N` fresh buckets with a primary agreement to `--provider` before the run. |
| `--prepare-max-bytes <BYTES>`| per-bucket load + 10% | Quota negotiated per prepared bucket. Rejected if below what the run writes to one bucket. |
| `--prepare-duration <BLOCKS>`| `100`              | Agreement duration for prepared buckets, in anchor blocks. |
| `--prepare-price-per-byte <P>`| `1`               | Price per byte per block the run accepts. The provider rejects lower offers and signs at its listed price. |
| `--bucket-source <MODE>`     | `create` if preparing, else `discover` | `discover`: existing buckets only. `create`: prepared buckets only. `auto`: existing first, prepare the shortfall up to `--prepare-buckets`. |

Total uploads is `users × uploads-per-user`. The two `--parallel-*` flags are
independent axes: `--parallel-users` alone runs each user's uploads in sequence
but the users concurrently, `--parallel-uploads` alone does the reverse, and both
together maximise concurrency (bounded by `--max-concurrency` if set).

**Behavior**

1. Derives the account from `--suri`/`--keyfile`. Unless `--bucket-source create`,
   reads its buckets from chain (`MemberBuckets[account]`) and keeps those with a
   `StorageAgreements[bucket][provider]` entry for the given `--provider`.
2. With `--prepare-buckets N`, negotiates terms with the provider node
   (`POST /negotiate`) and redeems them on-chain (`establish_storage_agreement`)
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
`just start-provider`, `cargo run -p storage-client --example register_provider`).
Buckets and agreements either exist already (default `discover` mode) or the run
opens them with `--prepare-buckets N`. The signing account pays for the prepared
agreements at the provider's listed price.

## Limitations

- **Agreement expiry is not checked.** A bucket is selected based on the presence
  of a `StorageAgreements[bucket][provider]` entry; expired-but-not-yet-cleared
  agreements are treated as matches.
