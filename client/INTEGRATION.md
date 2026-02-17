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
use storage_client::{AdminClient, ClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create config
    let config = ClientConfig {
        chain_ws_url: "ws://localhost:2222".to_string(),
        provider_urls: vec!["http://localhost:3333".to_string()],
        timeout_secs: 30,
        enable_retries: true,
    };

    // Create client and connect to chain
    let mut client = AdminClient::new(config, "5GrwvaEF...".to_string())?;
    client.base.connect_chain().await?;

    // Set signer (for testing)
    client.base = client.base.with_dev_signer("alice")?;

    // Now you can make on-chain calls
    let bucket_id = client.create_bucket(2).await?;
    println!("Created bucket: {}", bucket_id);

    Ok(())
}
```

### Using Custom Signers

For production, use actual keypairs instead of dev accounts:

```rust
use subxt_signer::sr25519::Keypair;

// Load from seed phrase or keystore
let keypair = Keypair::from_uri("//Alice")?;

// Set signer
if let Some(chain_client) = client.base.chain_client.as_mut() {
    let client = Arc::make_mut(chain_client);
    *client = client.clone().with_signer(keypair);
}
```

### Dynamic Extrinsics

The substrate module uses dynamic extrinsics (not requiring compile-time metadata):

```rust
// Example: Register provider
pub fn register_provider(multiaddr: Vec<u8>, public_key: Vec<u8>) -> impl Payload {
    subxt::dynamic::tx(
        "StorageProvider",       // Pallet name
        "register_provider",      // Call name
        vec![
            subxt::dynamic::Value::from_bytes(multiaddr),
            subxt::dynamic::Value::from_bytes(public_key),
        ],
    )
}
```

Benefits:
- No compile-time dependency on runtime metadata
- Works across runtime upgrades (as long as call signatures match)
- Easier to build and distribute

Drawbacks:
- Less type safety (errors at runtime instead of compile time)
- Must manually keep call signatures in sync with pallet

### Dynamic Storage Queries

Similarly, storage queries use dynamic access:

```rust
pub fn provider_info(account: &AccountId32) -> Address<subxt::dynamic::Value, (), (), ()> {
    subxt::dynamic::storage(
        "StorageProvider",    // Pallet name
        "Providers",          // Storage item name
        vec![subxt::dynamic::Value::from_bytes(account.as_ref())],
    )
}
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

### 1. Metadata Generation

For production, generate and include runtime metadata:

```bash
# Connect to your running node
subxt metadata -f bytes > client/metadata.scale
```

Then use the codegen macro in `substrate.rs`:

```rust
#[subxt::subxt(runtime_metadata_path = "metadata.scale")]
pub mod runtime {}
```

This provides:
- Compile-time type checking
- Auto-generated types for all pallets
- Better IDE support and documentation

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

        let mut client = ProviderClient::new(config, "5FHne...".to_string())?;
        client.base.connect_chain().await?;
        client.base = client.base.with_dev_signer("bob")?;

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

Ensure you call `connect_chain()` before making on-chain calls:

```rust
client.base.connect_chain().await?;
```

### "No signer configured" Error

Set a signer before submitting extrinsics:

```rust
client.base = client.base.with_dev_signer("alice")?;
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
