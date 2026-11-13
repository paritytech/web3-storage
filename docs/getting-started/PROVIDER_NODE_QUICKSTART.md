# Provider Node Quick Start Guide

This guide explains how to install `storage-provider-node`, register it as a storage provider on chain, and start it.
It is for the operator who runs the provider node.

The provider node is an HTTP server.
It stores uploaded data, builds commitments over the stored chunks, and answers on-chain challenges with proofs.
It connects to the storage parachain to read chain state and to submit transactions.

## 1. Hardware Requirements

| Resource | Requirement |
|----------|-------------|
| CPU | 64-bit CPU. Release binaries exist for Linux x86_64 and macOS arm64. A build from source compiles the full Rust dependency tree, so more cores reduce build time. |
| Disk | Persistent local disk for the storage directory. Size it for the capacity you offer to clients, plus room for the commitment state. |
| Network | A stable connection to a parachain RPC node, and an inbound port (default `3333`) that clients can reach. |
| Clock | System time synchronized with NTP. The node rejects client requests with a timestamp more than 300 seconds from its own clock. |

Do not use temporary storage for the data directory.
If the node loses its data, it cannot answer challenges for buckets that still have agreements with it.

## 2. Prerequisites and Installation

### Prerequisites

- `curl` and `jq`, for the download and the health check.

The node binary generates the provider key and submits the registration transactions, so no other tool is needed.

Install the node with one of the two options below.

### Option A: Download the release binary

Each release of the repository contains a prebuilt binary for these targets:

| Platform | Target |
|----------|--------|
| Linux x86_64 with glibc 2.30 or later | `x86_64-unknown-linux-gnu` |
| macOS arm64 | `aarch64-apple-darwin` |

Release tags have the form `v<version>-paseo`.
Set the release tag and your target, then download the archive and its checksum:

```bash
TAG=<release-tag>
TARGET=x86_64-unknown-linux-gnu
PKG=storage-provider-node-${TAG}-${TARGET}
curl -LO https://github.com/paritytech/web3-storage/releases/download/${TAG}/${PKG}.tar.gz
curl -LO https://github.com/paritytech/web3-storage/releases/download/${TAG}/${PKG}.tar.gz.sha256
```

Verify the checksum:

```bash
sha256sum -c ${PKG}.tar.gz.sha256       # Linux
shasum -a 256 -c ${PKG}.tar.gz.sha256   # macOS
```

Extract the archive and install the binary:

```bash
tar -xzf ${PKG}.tar.gz
sudo mkdir -p /usr/local/bin
sudo install -m 755 ${PKG}/storage-provider-node /usr/local/bin/
```

### Option B: Build from source

Additional prerequisites:

- Git
- Rust toolchain `1.93.0` with the `wasm32v1-none` target and the `clippy`, `rust-src` and `rustfmt` components.
  With `rustup` installed, the repository's `rust-toolchain.toml` selects this toolchain automatically.
- A C/C++ compiler and `clang`, required to build RocksDB.
- `just` (optional), for the shortcut commands in this guide: `cargo install just` or `brew install just`.

Clone the repository and build the provider node in release mode:

```bash
git clone https://github.com/paritytech/web3-storage.git
cd web3-storage
cargo build -p storage-provider-node --release
```

The same build is available as `just build-provider`.

The binary is written to `target/release/storage-provider-node`.
Install it:

```bash
sudo mkdir -p /usr/local/bin
sudo install -m 755 target/release/storage-provider-node /usr/local/bin/
```

### Check the installation

```bash
storage-provider-node --version
storage-provider-node --help
```

The rest of this guide runs the node as `storage-provider-node` from `/usr/local/bin`.

## 3. Key Preparation

The provider node uses one secret seed for two purposes:

- The sr25519 account derived from the seed is the provider's on-chain identity.
  The node submits all transactions from this account, and the account pays the stake and transaction fees.
- The signing key derived from the same seed signs commitments, checkpoint co-signatures, negotiated terms and replica sync attestations.
  The keyfile records the signing scheme: `sr25519` (default), `ed25519`, `ecdsa` or `eth`.
  `--key-scheme` overrides it.

### Generate the keyfile

The `helper generate-key` subcommand creates a new 12-word BIP39 secret phrase and writes it to the keyfile:

```bash
storage-provider-node helper \
  --chain-rpc wss://<parachain-rpc-host> \
  --keyfile ./secret/provider.key \
  generate-key
```

The command creates the file with owner-only permissions and never overwrites an existing file.
It reads the SS58 prefix from the chain so that the printed address matches the chain's own tools.
If the chain is not reachable, it prints a warning and uses the generic prefix 42.
`--words` selects another BIP39 length: 15, 18, 21 or 24.

The command prints the public fields of the new key:

```json
{
  "keyfile": "./secret/provider.key",
  "keyScheme": "sr25519",
  "accountId": "0x...",
  "publicKey": "0x...",
  "ss58Address": "5...",
  "ss58Prefix": 42
}
```

`ss58Address` is the provider account.
The secret phrase is only in the file.

The keyfile is a JSON file with `secretPhrase`, `keyScheme` and the four identity fields from the output.
It does not contain `keyfile`.
`secretPhrase`, `publicKey`, `accountId` and `ss58Address` use the field names following substrate best-practice.
The node also accepts a keyfile that contains only a plain secret phrase or derivation path, for example `//Alice` on a development chain.
The node does not start if the file is empty, or if the file is readable by group or others.

At the moment, the helper supports `sr25519` only.
To register with another scheme, write a plain-phrase keyfile and register with the example program described in [Register with the example program](#register-with-the-example-program).

### Fund the account

Transfer enough balance to the provider account to cover the registration stake and transaction fees.
The minimum stake is 1000 tokens.
The stake also limits the capacity you can offer: each byte of capacity requires 1000 planck of stake, so 1000 tokens back 1 TB.

### Key rules

- Keep the keyfile secret and back it up.
  Anyone with the secret phrase controls the provider account and the stake.
- The signing key registered on chain cannot be changed.
  The node signs with the scheme recorded in the keyfile, or with `--key-scheme` when given, and that scheme must match the one you register.
  If they differ, every signing endpoint returns `503 provider_key_mismatch`.
  To fix a mismatch, start the node with the original keyfile and scheme.

## 4. Register as Provider

Registration has two transactions, both signed by the provider account:

1. `StorageProvider.register_provider(multiaddr, public_key, stake)`
   - `multiaddr`: the address clients use to reach the node. Pass the same value as `--public-multiaddr` when you start the node.
   - `public_key`: the public signing key derived from the secret phrase for the scheme in the keyfile.
   - `stake`: at least 1000 tokens.
2. `StorageProvider.update_provider_settings(settings)`
   - `min_duration` and `max_duration`: the agreement durations you accept, in relay chain blocks.
   - `price_per_byte`: the price in planck per byte per relay chain block.
   - `accepting_primary`: whether you accept new primary agreements.
   - `replica_sync_price`: the price in planck per sync confirmation, or none if you do not accept replica agreements.
   - `accepting_extensions`: whether you accept extensions of existing agreements.
   - `max_capacity`: the maximum capacity in bytes, or `0` for no limit, covered by your stake.
   - `min_agreement_bytes`: the smallest agreement size in bytes you accept, or `0` for no minimum.

The `helper` subcommand of the node binary submits each transaction.
It signs with the account derived from `--keyfile`, validates the input and the provider's on-chain state before it submits, and exits with a non-zero status and the error message on failure.
It does not need a running node.
`--chain-rpc` selects the chain, as for the node.

### Register

Submit `register_provider` with the multiaddr and the stake in planck.
The chain has 12 decimals, so the minimum stake of 1000 tokens is `1000000000000000` planck:

```bash
storage-provider-node helper \
  --chain-rpc wss://<parachain-rpc-host> \
  --keyfile ./secret/provider.key \
  register \
  --multiaddr /dns4/<your-domain>/tcp/443/tls/http \
  --stake 1000000000000000
```

The command prints the account, the multiaddr, the stake and the block that included the transaction.
It rejects an empty multiaddr, a zero stake and an account that is already registered.
The chain rejects a stake below the minimum and a multiaddr above its length limit.

Registration applies the default settings: no price, no duration limits, no capacity limit, no minimum agreement size, primary agreements and extensions accepted, replica agreements not accepted.

### Set the provider settings

Submit `update_provider_settings` with the settings of your offer that differ from the defaults.
Only the given flags change; every other setting keeps its on-chain value.

```bash
storage-provider-node helper \
  --chain-rpc wss://<parachain-rpc-host> \
  --keyfile ./secret/provider.key \
  update-settings \
  --price-per-byte 1 \
  --min-duration 10 \
  --max-duration 100000 \
  --max-capacity 1000000000000
```

| Flag | Setting |
|------|---------|
| `--min-duration <BLOCKS>` | `min_duration` |
| `--max-duration <BLOCKS>` | `max_duration` |
| `--price-per-byte <AMOUNT>` | `price_per_byte` |
| `--accepting-primary <BOOL>` | `accepting_primary` |
| `--replica-sync-price <AMOUNT>` | `replica_sync_price`; setting it accepts replica agreements |
| `--no-replicas` | `replica_sync_price = none`; stops accepting replica agreements |
| `--accepting-extensions <BOOL>` | `accepting_extensions` |
| `--max-capacity <BYTES>` | `max_capacity` |
| `--min-agreement-bytes <BYTES>` | `min_agreement_bytes` |

The command prints the settings that are on chain after the update.
If the merged settings equal the current ones, it submits nothing.
Before it submits, it rejects a `min_duration` above `max_duration`.
When `max_capacity` is not `0`, it also rejects a `max_capacity` below the bytes already committed, a `min_agreement_bytes` above `max_capacity`, and a `max_capacity` that the stake does not cover.

Run the same command later to change your offer.

### Register with the example program

The repository also contains a program that runs both transactions and supports every key scheme.
It runs from a source checkout and needs the Rust toolchain from Option B.
It reads the keyfile as a plain secret phrase and does not parse the JSON keyfile from `helper generate-key`.
It derives the public key from the phrase in the same way as the node, and it does nothing if the account is already registered with the same key.

```bash
cargo run -p storage-client --example register_provider -- \
  wss://<parachain-rpc-host> \
  https://<your-domain> \
  /dns4/<your-domain>/tcp/443/tls/http \
  ./secret/provider.key \
  sr25519
```

The arguments are, in order: chain RPC URL, provider HTTP URL, multiaddr, keyfile and key scheme.
The program stakes 1000 tokens and applies these settings: `price_per_byte = 1`, `min_duration = 10`, `max_duration = 100000`, `accepting_primary = true`, `accepting_extensions = true`, no replica agreements, `max_capacity = 0`.
After registration, change the settings with `helper update-settings` to match your offer.

On a local development chain, `just register-provider` registers the `//Alice` development account.

## 5. Start and Configure

### Start the node

```bash
storage-provider-node \
  --keyfile ./secret/provider.key \
  --chain-rpc wss://<parachain-rpc-host> \
  --storage-path /var/lib/provider-data \
  --bind-addr 0.0.0.0:3333 \
  --public-multiaddr /dns4/<your-domain>/tcp/443/tls/http \
  --enable-challenge-responder
```

`--public-multiaddr` must be the multiaddr you registered in [Register](#register).
The options below add CORS restrictions, replica sync and other settings.

On startup the node logs the storage backend, the signing account, and the address it listens on:

```text
Signing enabled for account: 5DPh... (scheme: Sr25519)
Starting storage provider node on 0.0.0.0:3333
```

If the chain is not reachable at startup, the node logs a warning and keeps retrying in the background.

In a source checkout, for a local development chain, the shortcut `just start-provider` builds and starts the node with the `//Alice` development key, connected to `ws://127.0.0.1:2222`.
Pass your own key with `just PROVIDER_KEYFILE=./secret/provider.key start-provider`.

### Chain RPC endpoint

| Option | Environment variable | Default | Description |
|--------|----------------------|---------|-------------|
| `--chain-rpc` | `CHAIN_RPC` | `ws://127.0.0.1:2222` | WebSocket URL of the parachain RPC node. |
| `--chain-transport` | `CHAIN_TRANSPORT` | `rpc` | `rpc` connects to the node at `--chain-rpc`. `light` runs an embedded light client and needs no RPC node when both chain specs are files. |
| `--relay-chain-spec` | `RELAY_CHAIN_SPEC` | none | Relay chain spec for `light`: a raw spec file path, or a `ws://`/`wss://` URL to fetch it from. Required for `light`. |
| `--para-chain-spec` | `PARA_CHAIN_SPEC` | fetched from `--chain-rpc` | Parachain spec for `light`: a raw spec file path, or a `ws://`/`wss://` URL. |

Fetching a chain spec from a URL trusts that node.
In production, use spec files when you run the light client.

### HTTP endpoint

| Option | Environment variable | Default | Description |
|--------|----------------------|---------|-------------|
| `--bind-addr` | `BIND_ADDR` | `0.0.0.0:3333` | Address and port of the HTTP server. |
| `--public-multiaddr` | `PUBLIC_MULTIADDR` | derived from `--bind-addr` | Address that clients use to reach the node. |
| `--cors-allowed-origins` | `CORS_ALLOWED_ORIGINS` | all origins | Comma-separated browser origins allowed to call the node. |

Without `--public-multiaddr` and with the default `--bind-addr`, the node advertises `/ip4/127.0.0.1/tcp/<port>`, which clients outside the host cannot reach.
On a public deployment, put the node behind a TLS reverse proxy and set `--public-multiaddr` to the proxy address.
Set `--cors-allowed-origins` in production.

### Key and storage

| Option | Environment variable | Default | Description |
|--------|----------------------|---------|-------------|
| `--keyfile` | none | none | Path to the keyfile: the JSON file from `helper generate-key`, or a plain secret phrase. Without it, the node cannot sign or submit transactions. |
| `--key-scheme` | none | `keyScheme` from the keyfile, else `sr25519` | Signing scheme: `sr25519`, `ed25519`, `ecdsa` or `eth`. |
| `--storage-backend` | none | `rocksdb` | Storage engine. |
| `--storage-path` | `STORAGE_PATH` | `./provider-data` | Directory for chunks and commitment state. |

### Background services

| Option | Environment variable | Default | Description |
|--------|----------------------|---------|-------------|
| `--enable-challenge-responder` | `ENABLE_CHALLENGE_RESPONDER` | off | Answer on-chain challenges automatically. |
| `--challenge-poll-interval` | `CHALLENGE_POLL_INTERVAL` | `300` | Seconds between reconciliation scans for challenges. `0` disables the scan. |
| `--enable-replica-sync` | `ENABLE_REPLICA_SYNC` | off | Sync buckets for which this provider is a replica. |
| `--replica-poll-interval` | `REPLICA_POLL_INTERVAL` | `600` | Seconds between reconciliation passes for replica duties. `0` disables the pass. |
| `--replica-sync-timeout` | `REPLICA_SYNC_TIMEOUT` | `300` | Seconds before a replica sync operation times out. |
| `--replica-max-concurrent` | `REPLICA_MAX_CONCURRENT` | `3` | Maximum number of buckets synced at the same time. |

A provider that does not answer a challenge in time is slashed.
Enable the challenge responder unless another service answers challenges for you.

### Authentication

| Option | Environment variable | Default | Description |
|--------|----------------------|---------|-------------|
| `--auth-cache-ttl` | `AUTH_CACHE_TTL` | `30` | Seconds a bucket membership lookup stays cached. |
| `--auth-max-skew` | `AUTH_MAX_SKEW` | `300` | Maximum difference in seconds between a request timestamp and the node clock. |
| `--auth-max-stale` | `AUTH_MAX_STALE` | `300` | Maximum age in seconds of cached membership used while the chain is unreachable. |
| `--auth-cache-max-entries` | `AUTH_CACHE_MAX_ENTRIES` | `10000` | Maximum number of buckets in the membership cache. |

The defaults are suitable for most deployments.

## 6. Health Check

Check that the HTTP server is running:

```bash
curl -s http://127.0.0.1:3333/health | jq .
```

```json
{
  "status": "healthy",
  "version": "0.4.1"
}
```

Check the node identity and readiness:

```bash
curl -s http://127.0.0.1:3333/info | jq .
```

The `readiness` object reports:

| Field | Meaning |
|-------|---------|
| `signing_configured` | `true` when the node started with a keyfile. |
| `provider_info_loaded` | `true` when the node found its registration on chain. It is `false` when the provider is not registered. |
| `deregistering` | `true` when the provider has announced deregistration. |

Check the stored data:

```bash
curl -s http://127.0.0.1:3333/stats | jq .
```

The response contains the provider account, the number of buckets, the number of stored nodes and the total stored bytes.

On a local setup, `just health` and `just stats` run the same requests.

### Verify the registration

The node reads its registration from the chain at startup.
Query `/info`:

```bash
curl -s http://127.0.0.1:3333/info | jq .readiness
```

`provider_info_loaded` is `true` when the registration is on chain.
`provider_registration_info` shows the registered multiaddr, stake and settings.

At startup, the node compares its multiaddr with the one on chain and updates the on-chain value if they differ.
This check runs only when the node has a keyfile, the chain is reachable at startup, and the provider is registered.
After the first registration, restart the node if the registered multiaddr differs from `--public-multiaddr`.
