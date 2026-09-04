# Storage Provider Node

Off-chain HTTP server that handles data upload and download, builds MMR
commitments over stored chunks, and responds to on-chain challenges with
inclusion proofs.

## Build & Run

```bash
cargo build -p storage-provider-node --release
just start-provider   # requires a running chain (just start-chain)
just health           # check provider is up
```

## Storage backend

`--storage-backend` picks the storage engine (default `rocksdb`). Chunks, MMR
state and the nonce counter go under `--storage-path` (`./provider-data`, or
`$STORAGE_PATH`) and survive a restart — a provider that forgot its data could
not answer challenges for buckets it still has agreements for.

Writes are not fsynced (RocksDB's default `WriteOptions`), so what they survive
is a clean process restart, not a power loss or kernel panic. `DiskNonceStore`
documents what that costs the nonce counter.

## Authentication

Authentication is always enforced: every mutating Layer-0 endpoint (`PUT
/node`, `POST /commit`, `POST /delete`) and all fs/s3 endpoints require a
signed `Authorization` header, and there is no way to turn enforcement off.
Layer-0 content-addressed reads (`GET /node`, `GET /read`, `POST
/fetch_nodes`, the commitment/proof endpoints) are currently unauthenticated;
whether they should require the `Reader` role is tracked in
[#228](https://github.com/paritytech/web3-storage/issues/228).

The client signs an sr25519 message binding the request to a bucket and a
timestamp:

```text
signed message:  web3storage:<METHOD>:<bucket_id>:<timestamp>
header:          Authorization: Web3Storage <pubkey_hex>:<signature_hex>:<timestamp>
```

| Field          | Meaning                                                                 |
| -------------- | ----------------------------------------------------------------------- |
| `METHOD`       | Upper-case HTTP verb of the request (`GET`, `PUT`, `POST`, `DELETE`).   |
| `bucket_id`    | Decimal id of the bucket the request targets.                           |
| `timestamp`    | Client Unix time in **seconds**; identical in the message and header.   |
| `pubkey_hex`   | 32-byte sr25519 public key, hex (optional `0x` prefix).                 |
| `signature_hex`| 64-byte signature over the message, hex (optional `0x` prefix).         |

The recovered public key is mapped to the bucket's on-chain role
(`Reader` / `Writer` / `Admin`); reads need `Reader`, writes and deletes need
`Writer`, and pruning (`POST /delete`) needs `Admin`. The `timestamp` must be
within the configured skew window of the provider's clock or the request is
rejected as expired.

Bucket roles are cached, not read fresh on every request. A membership change
takes effect on the first request after the finalized block that carries it.
A feed that lags or reconnects invalidates every cached bucket immediately,
rather than waiting on the TTL; only a feed that stops running entirely (no
chain-state coordinator) falls back to `--auth-cache-ttl` (default 30s). If
the chain is unreachable when a lookup needs to refetch, the cached member
set is still served, but only for up to `--auth-max-stale` (default 5
minutes) - past that, the request is refused with `503`.

A bucket the provider does not know yet is given `--auth-finality-grace`
(default 12s) for its creation event to arrive before the request is refused
with `403`: a client that has just watched `create_bucket` finalize on its own
node can be a finality step ahead of a provider on the embedded light client,
which learns finality a few seconds later. `0` refuses at once.

Because any keypair can ask about any bucket id, the cache is also capped at
`--auth-cache-max-entries` buckets (default 10,000), and entries are removed
rather than left stale: a member set at the stale bound, an empty one already
at the TTL. Eviction costs nothing but a re-resolve on the next request.

## Test

```bash
cargo test -p storage-provider-node
```

## License

Licensed under [GPL-3.0-only](../LICENSE-GPL3).
