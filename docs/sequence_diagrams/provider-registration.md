# Provider Registration

End-to-end flow from the provider UI wizard through on-chain registration, settings configuration, and provider node startup.

## 1. On-Chain Registration (Two Transactions)

```mermaid
sequenceDiagram
    autonumber
    participant Provider as Provider (Wallet)
    participant Chain as Parachain (StorageProvider Pallet)

    Provider->>Chain: register_provider(<br/>multiaddr: "/ip4/127.0.0.1/tcp/3333",<br/>public_key: sr25519 (32 bytes),<br/>stake: 1000+ tokens)

    Chain->>Chain: Ensure not already registered
    Chain->>Chain: Ensure stake >= MinProviderStake (1000 tokens)
    Chain->>Chain: Validate public_key length (32, 33, or 64 bytes)
    Chain->>Chain: validate_settings(defaults, committed=0, stake)

    Chain->>Chain: Currency::reserve(provider, stake)<br/>Tokens moved: free balance -> reserved balance
    Chain->>Chain: Create ProviderInfo{multiaddr, public_key,<br/>stake, committed_bytes=0,<br/>settings=default, stats={registered_at=now}}
    Chain->>Chain: Insert into Providers storage map
    Chain->>Chain: Insert default ProviderReplayStates (nonce window)

    Chain-->>Provider: Event: ProviderRegistered{provider, stake}

    Note over Provider,Chain: Immediately update settings

    Provider->>Chain: update_provider_settings({<br/>min_duration, max_duration,<br/>price_per_byte, accepting_primary: true,<br/>replica_sync_price, accepting_extensions,<br/>max_capacity})

    Chain->>Chain: Validate min_duration <= max_duration
    Chain->>Chain: Ensure provider not in deregister state
    Chain->>Chain: validate_settings: if max_capacity > 0,<br/>stake >= max_capacity * MinStakePerByte

    Chain->>Chain: Update ProviderInfo.settings
    Chain-->>Provider: Event: ProviderSettingsUpdated{provider, settings}

    Note over Provider: Provider now discoverable via<br/>find_matching_providers runtime API
```

## 2. Provider Node Startup

```mermaid
sequenceDiagram
    autonumber
    participant Node as Provider Node (CLI)
    participant Chain as Parachain

    Note over Node: storage-provider-node --keyfile key.txt<br/>--chain-rpc ws://... --bind-addr 0.0.0.0:3333

    Node->>Node: Load seed from keyfile (chmod 600)
    Node->>Node: Derive Sr25519 keypair -> SS58 account ID
    Node->>Node: Create storage backend (in-memory or disk)

    Node->>Chain: Read Providers storage for this account
    alt Not registered
        Node->>Node: Log warning: "register it before starting"
    else Registered
        Node->>Node: Load settings (price, capacity, etc.)
    end

    Node->>Chain: Read ProviderReplayStates.hsn
    Node->>Node: Bootstrap nonce counter (hsn + 1)

    opt --enable-checkpoint-coordinator
        Node->>Node: Start checkpoint coordinator (polls every 6s)
    end

    opt Replica sync enabled
        Node->>Node: Start replica sync coordinator
    end

    Node->>Chain: Read on-chain multiaddr
    alt On-chain multiaddr != bind/public addr
        Node->>Chain: update_provider_multiaddr(new_multiaddr)
        Chain-->>Node: Event: ProviderMultiaddrUpdated
    end

    Node->>Node: Start HTTP server on bind address

    Note over Node: Ready to accept uploads,<br/>serve downloads, respond to challenges
```

## 3. Adding Stake

```mermaid
sequenceDiagram
    autonumber
    participant Provider as Provider (Wallet)
    participant Chain as Parachain

    Provider->>Chain: add_provider_stake(amount)
    Chain->>Chain: Ensure provider registered
    Chain->>Chain: Ensure not in deregister state
    Chain->>Chain: Currency::reserve(provider, amount)
    Chain->>Chain: provider.stake += amount
    Chain-->>Provider: Event: ProviderStakeAdded{provider, amount, total_stake}
```

## Key Types

### ProviderInfo (on-chain)

| Field | Type | Description |
|-------|------|-------------|
| `multiaddr` | `BoundedVec<u8>` | Network address, e.g. `/ip4/127.0.0.1/tcp/3333` |
| `public_key` | `BoundedVec<u8, 64>` | Sr25519 (32b), Ed25519 (32b), or Ecdsa (33/64b) |
| `stake` | `Balance` | Locked collateral |
| `committed_bytes` | `u64` | Sum of `max_bytes` across active agreements |
| `settings` | `ProviderSettings` | Pricing and availability config |
| `stats` | `ProviderStats` | On-chain reputation metrics |
| `deregister_at` | `Option<BlockNumber>` | Set during two-step exit |

### ProviderSettings

| Field | Default | Description |
|-------|---------|-------------|
| `min_duration` | 0 | Minimum agreement duration (blocks) |
| `max_duration` | MAX | Maximum agreement duration (blocks) |
| `price_per_byte` | 0 | Price per byte per block |
| `accepting_primary` | true | Accepting new primary agreements |
| `replica_sync_price` | None | None = not accepting replicas |
| `accepting_extensions` | true | Accepting agreement extensions |
| `max_capacity` | 0 | 0 = unlimited |

### Stake Requirements

```
MinProviderStake = 1000 tokens (absolute minimum)
MinStakePerByte  = 1000 units per byte of declared capacity

Required stake = max(MinProviderStake, max_capacity * MinStakePerByte)
```

If `max_capacity = 0` (unlimited), only `MinProviderStake` applies.

## Storage Items Created

| Storage Item | Key | Value |
|-------------|-----|-------|
| `Providers` | `AccountId` | `ProviderInfo { multiaddr, public_key, stake, settings, stats, ... }` |
| `ProviderReplayStates` | `AccountId` | `ReplayWindow` (nonce sliding window for agreement signing) |

## Events

| Event | When |
|-------|------|
| `ProviderRegistered { provider, stake }` | Registration succeeds |
| `ProviderSettingsUpdated { provider, settings }` | Settings changed |
| `ProviderMultiaddrUpdated { provider }` | Network address changed |
| `ProviderStakeAdded { provider, amount, total_stake }` | Additional stake locked |
