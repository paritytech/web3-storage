# Substrate Integration Guide

This document explains how the client SDK integrates with the Substrate blockchain.

## Architecture

The SDK uses [subxt](https://github.com/paritytech/subxt) for blockchain interactions. The integration is organized in layers:

### 1. Substrate Client (`src/substrate.rs`)

Wraps subxt's `OnlineClient` and provides:
- Connection management to substrate nodes
- Signer management (dev accounts and custom keypairs)
- Dynamic extrinsic builders for pallet calls
- Dynamic storage query builders

### 2. Base Client (`src/base.rs`)

Provides common functionality:
- HTTP client for provider nodes
- Substrate client access
- Configuration management
- Helper utilities

### 3. Specialized Clients

Each specialized client (ProviderClient, AdminClient, etc.) uses the base client to:
- Submit extrinsics via `chain().api().tx().sign_and_submit_then_watch_default()`
- Query storage via `chain().api().storage()`
- Parse events from transaction results

## Usage

### Basic Setup

```rust
use storage_client::{AdminClient, ClientConfig, Signer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create config
    let config = ClientConfig {
        chain_ws_url: "ws://localhost:2222".to_string(),
        provider_urls: vec!["http://localhost:3333".to_string()],
        timeout_secs: 30,
        enable_retries: true,
    };

    // Create client (dev signer for testing) and connect to chain
    let mut client = AdminClient::new(config, Signer::from_seed("//Alice")?)?;
    client.connect().await?;

    // Now you can make on-chain calls
    let bucket_id = client.create_bucket(2).await?;
    println!("Created bucket: {}", bucket_id);

    Ok(())
}
```

### Using Custom Signers

For production, use actual keypairs instead of dev accounts:

```rust
use storage_client::{AdminClient, Signer};
use subxt_signer::sr25519::Keypair;

// Load from seed phrase or keystore
let keypair = Keypair::from_uri("//Alice")?;

// Any subxt sr25519 keypair converts into a Signer. This is the
// extrinsic-submission account; the provider's *signing* key registered as
// `public_key` may use any supported scheme (sr25519/ed25519/ecdsa/eth —
// see the register_provider example's `scheme` argument).
let mut client = AdminClient::new(config, Signer::from(keypair))?;
client.connect().await?;
```

### Static Extrinsics

The substrate module builds extrinsics through the generated `storage-subxt`
bindings, so every call is checked against the runtime metadata at compile
time. `src/convert.rs` holds the conversions between SDK types
(`sp_runtime`, `storage_primitives`) and the generated runtime types:

```rust
// Example: Register provider (src/substrate.rs)
pub fn register_provider(multiaddr: Vec<u8>, public_key: Vec<u8>, stake: u128) -> impl Payload {
    api::tx().storage_provider().register_provider(
        convert::bounded(multiaddr),
        convert::bounded(public_key),
        stake,
    )
}
```

Benefits:
- Compile-time type checking against the runtime metadata
- Typed events, storage values, and storage keys — no manual decoding
- Runtime drift surfaces as a hard `IncompatibleCodegen` error instead of
  silently mis-decoded data

Trade-off: after a runtime change, regenerate the bindings with
`just subxt-codegen` (needs a running chain).

### Static Storage Queries

Storage reads use the generated typed addresses directly:

```rust
let info = at
    .storage()
    .try_fetch(
        api::storage().storage_provider().providers(),
        (convert::account(&account_id),),
    )
    .await?; // -> Option<pallet::ProviderInfo>, fully typed
```

## Implementation Status

### ✅ Implemented

- **ProviderClient**:
  - `register()` - Register as storage provider
  - `accept_agreement()` - Accept storage agreement
  - `respond_to_challenge()` - Respond to data challenge

- **AdminClient**:
  - `create_bucket()` - Create new bucket
  - `request_agreement()` - Request storage from provider

- **ChallengerClient**:
  - `challenge_checkpoint()` - Challenge provider data

### 🚧 Partially Implemented

Methods marked with `// TODO: Submit extrinsic` still use placeholder logic but have the infrastructure in place. To complete them:

1. Add the extrinsic builder to `src/substrate.rs::extrinsics`
2. Update the client method to call the builder
3. Submit and wait for finalization

### 📋 TODO

- **Event Parsing**: Extract data from transaction events (e.g., bucket IDs, challenge IDs)
- **Storage Queries**: Implement query methods for reading on-chain state
- **Runtime API Calls**: Use the custom Runtime API for complex queries
- **Error Handling**: Map substrate errors to ClientError variants
- **Batch Operations**: Support submitting multiple extrinsics in one transaction

## Production Considerations

### 1. Keeping Bindings Fresh

The generated bindings live in `crates/storage-subxt` (checked-in
`subxt codegen` output). After any runtime change, regenerate them:

```bash
# Terminal 1: run a chain with the new runtime
just start-paseo-chain

# Terminal 2: refresh metadata + generated code
just subxt-codegen
```

Static payloads and addresses carry validation hashes, so stale bindings
fail fast at submission/query time rather than mis-decoding.

### 2. Signer Security

- Never hardcode private keys
- Use keystore files or hardware wallets
- Implement proper key rotation
- Consider using proxy accounts for operations

### 3. Error Handling

Current implementation uses basic error mapping. For production:
- Parse specific substrate errors
- Retry transient failures
- Handle nonce issues
- Monitor finalization delays

### 4. Transaction Monitoring

Current implementation waits for finalization. Consider:
- Using `wait_for_in_block()` for faster confirmation
- Implementing transaction status callbacks
- Tracking transaction lifetime and expiry
- Handling transaction drops

### 5. Connection Management

- Implement reconnection logic
- Handle node upgrades gracefully
- Support multiple endpoint failover
- Monitor connection health

## Testing

### Unit Tests

Mock the substrate client for testing business logic:

```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_register_provider() {
        // Use testnet or local dev chain
        let config = ClientConfig {
            chain_ws_url: "ws://localhost:2222".to_string(),
            // ...
        };

        let mut client = ProviderClient::new(config, Signer::from_seed("//Bob")?)?;
        client.connect().await?;

        let result = client.register(
            "/ip4/127.0.0.1/tcp/3333".to_string(),
            vec![0u8; 32],
            1_000_000_000_000,
        ).await;

        assert!(result.is_ok());
    }
}
```

### Integration Tests

Run against a local development chain:

```bash
# Terminal 1: Start local node
cargo build --release
./target/release/storage-parachain-node --dev

# Terminal 2: Run tests
cd client
cargo test --features integration-tests
```

## Troubleshooting

### "Not connected to chain" Error

Ensure you call `connect()` before making on-chain calls:

```rust
client.connect().await?;
```

### "No signer configured" Error

Every client takes its signer at construction — pass one to `new()` (or
`with_defaults()`):

```rust
let client = AdminClient::new(config, Signer::from_seed("//Alice")?)?;
```

### Transaction Fails

Check:
1. Account has sufficient balance for fees
2. Pallet call name matches runtime
3. Arguments match pallet call signature
4. Nonce is correct (usually handled automatically)

### Connection Timeouts

Increase timeout in config:

```rust
let config = ClientConfig {
    timeout_secs: 60,  // Increase from default 30
    // ...
};
```

## Further Reading

- [Subxt Documentation](https://docs.rs/subxt/)
- [Polkadot SDK Documentation](https://paritytech.github.io/polkadot-sdk/)
- [Substrate Node Template](https://github.com/substrate-developer-hub/substrate-node-template)
