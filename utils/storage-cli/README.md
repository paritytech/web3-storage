# storage-cli

Storage CLI for [scalable Web3 storage](../../README.md). It consolidates the
on-chain and off-chain storage operations that were previously scattered across
`client/examples/*` and ad-hoc scripts into a single ergonomic binary, built on
top of the [`storage-client`](../../client) SDK.

## Usage

```bash
cargo run -p storage-cli -- --help
cargo run -p storage-cli -- stress-test upload --help
```

### Global flags

| Flag                 | Default                  | Env            | Description                                  |
| -------------------- | ------------------------ | -------------- | -------------------------------------------- |
| `--chain-rpc <URL>`  | `ws://127.0.0.1:2222`    | `CHAIN_RPC`    | Parachain RPC WebSocket endpoint.            |
| `--provider-url <URL>` | `http://127.0.0.1:3333` | `PROVIDER_URL` | Provider node HTTP endpoint.                 |
| `--suri <SURI>`      | —                        |                | Secret URI for the account, e.g. `//Alice`.  |
| `--keyfile <FILE>`   | —                        |                | File whose contents are the SURI/seed.       |

`--suri` and `--keyfile` are mutually exclusive; exactly one is required.

## `stress-test upload`

Uploads generated data to every bucket the account **already** has a storage
agreement with the given provider for.

```bash
cargo run -p storage-cli -- \
  --suri //Bob \
  stress-test upload --provider <PROVIDER_SS58> --size 4096
```

| Param                        | Default     | Description                                          |
| ---------------------------- | ----------- | ---------------------------------------------------- |
| `--provider <ACCOUNT>`       | required    | Provider account (SS58 or `0x`-hex) to target.       |
| `--max-buckets-to-write <N>` | all buckets | Cap the number of buckets written to.                |
| `--size <BYTES>`             | `1048576`   | Bytes of generated data to upload per bucket.        |

**Behavior**

1. Derives the account from `--suri`/`--keyfile` and reads its buckets from chain
   (`MemberBuckets[account]`).
2. Keeps only buckets that have a `StorageAgreements[bucket][provider]` entry for
   the given `--provider`.
3. If none match, it exits with an error — it does **not** create any bucket or
   agreement.
4. Uploads generated data to each selected bucket over the provider's HTTP API.

### Required on-chain setup

The command only writes to buckets that already have an agreement, so the agreement
must exist first. With a chain and provider running (`just start-chain`,
`just start-provider`), open one — for example via the SDK example:

```bash
cargo run -p storage-client --example complete_workflow -- \
  ws://127.0.0.1:2222 http://127.0.0.1:3333 //Bob
```

That negotiates terms and establishes an agreement, creating a bucket owned by
`//Bob` with a primary agreement to the provider. Run `stress-test upload` as the
same account (`--suri //Bob`) targeting that provider.

## Limitations

- **Agreement expiry is not checked.** A bucket is selected based on the presence
  of a `StorageAgreements[bucket][provider]` entry; expired-but-not-yet-cleared
  agreements are treated as matches.
