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

## Signing key

The node signs commitments, checkpoint co-signatures, negotiated terms, and
replica sync attestations with the keypair derived from `--keyfile`. The
scheme is selected with `--key-scheme` (`sr25519` default, `ed25519`,
`ecdsa`, or `eth`) and must match the `public_key` registered on-chain —
`/negotiate` returns `503 provider_key_mismatch` while they differ.
Extrinsics are always submitted from the sr25519 account derived from the
same seed, which is the provider's on-chain identity.

Every signature leaves the node as a SCALE-encoded `MultiSignature`,
`0x`-prefixed hex, so the scheme tag travels with it (e.g.
`0x01<64-byte sr25519 sig>`, `0x02<65-byte ecdsa sig>`).

## Authentication

Authentication is always enforced: every mutating Layer-0 endpoint (`PUT
/node`, `POST /commit`, `POST /delete`) and all fs/s3 endpoints require a
signed `Authorization` header, and there is no way to turn enforcement off.
Layer-0 content-addressed reads (`GET /node`, `GET /read`, `POST
/fetch_nodes`, the commitment/proof endpoints) are currently unauthenticated;
whether they should require the `Reader` role is tracked in
[#228](https://github.com/paritytech/web3-storage/issues/228).

The client signs an sr25519 message binding the request to a bucket and a
timestamp. Client auth is sr25519-only for now (unlike on-chain provider
signature verification, which is multi-scheme); extending it is part of the
header redesign tracked in
[#304](https://github.com/paritytech/web3-storage/issues/304):

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

## Test

```bash
cargo test -p storage-provider-node
```

## License

Licensed under [GPL-3.0-only](../LICENSE-GPL3).
