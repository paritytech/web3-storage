# Storage Marketplace Design

## Overview

The Storage Marketplace enables automatic provider discovery and matching based on user storage requirements. Instead of manually selecting providers, users can specify their needs and the system finds suitable providers.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Storage Marketplace                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────────┐    ┌──────────────────┐    ┌───────────────┐  │
│  │    Provider      │    │    Matching      │    │   Discovery   │  │
│  │   Registration   │───▶│     Engine       │◀───│     Client    │  │
│  │                  │    │                  │    │               │  │
│  └──────────────────┘    └──────────────────┘    └───────────────┘  │
│          │                       │                       │          │
│          ▼                       ▼                       ▼          │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                      On-Chain Storage                         │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐   │   │
│  │  │  Provider   │  │   Bucket    │  │     Agreement       │   │   │
│  │  │    Info     │  │    Info     │  │      Requests       │   │   │
│  │  └─────────────┘  └─────────────┘  └─────────────────────┘   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

## Core Concepts

### 1. Provider Capacity Declaration

Providers declare their maximum storage capacity when registering or updating settings:

```rust
pub struct ProviderSettings<T: Config> {
    pub min_duration: BlockNumberFor<T>,      // Min agreement duration
    pub max_duration: BlockNumberFor<T>,      // Max agreement duration
    pub price_per_byte: BalanceOf<T>,         // Price per byte per block
    pub accepting_primary: bool,              // Accepting new agreements
    pub replica_sync_price: Option<BalanceOf<T>>, // Replica pricing
    pub accepting_extensions: bool,           // Accepting extensions
    pub max_capacity: u64,                    // NEW: Max bytes (0 = unlimited)
}
```

**Key Rules:**
- `max_capacity = 0` means unlimited capacity (backward compatible)
- `max_capacity` cannot be set below current `committed_bytes`
- Provider must have sufficient stake: `stake >= max_capacity * MinStakePerByte`

### 2. Capacity Enforcement

When a provider accepts an agreement, the system enforces capacity limits:

```
                    ┌─────────────────────────┐
                    │   Accept Agreement      │
                    └───────────┬─────────────┘
                                │
                    ┌───────────▼─────────────┐
                    │  Calculate new_committed │
                    │  = committed + requested │
                    └───────────┬─────────────┘
                                │
              ┌─────────────────┼─────────────────┐
              │                 │                 │
    ┌─────────▼─────────┐       │       ┌─────────▼─────────┐
    │ max_capacity > 0? │       │       │ Check stake ratio │
    └─────────┬─────────┘       │       └─────────┬─────────┘
              │                 │                 │
       Yes    │                 │                 │
              ▼                 │                 ▼
    ┌─────────────────────┐     │     ┌─────────────────────┐
    │ new_committed <=    │     │     │ stake >= required   │
    │ max_capacity?       │     │     │ stake for bytes?    │
    └─────────┬───────────┘     │     └─────────┬───────────┘
              │                 │                 │
        No    │   Yes           │           No    │   Yes
              ▼                 │                 ▼
    ┌─────────────────┐         │     ┌─────────────────┐
    │ CapacityExceeded│         │     │InsufficientStake│
    └─────────────────┘         │     └─────────────────┘
                                │
                    ┌───────────▼─────────────┐
                    │    Agreement Created    │
                    └─────────────────────────┘
```

### 3. Storage Requirements

Users specify their needs when searching for providers:

```rust
pub struct StorageRequirements {
    pub bytes_needed: u64,        // Required storage capacity
    pub min_duration: u32,        // Minimum agreement duration
    pub max_price_per_byte: u128, // Maximum acceptable price
    pub primary_only: bool,       // Only match primary providers
}
```

### 4. Provider Matching

The matching engine scores providers based on how well they meet requirements:

```
Score Calculation:
┌─────────────────────────────────────────────────────────────┐
│                    Base Score: 100                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Check 1: Accepting Status                                  │
│  ├─ Not accepting → Score = 0, reason = NotAccepting        │
│  └─ Accepting → Continue                                    │
│                                                             │
│  Check 2: Capacity                                          │
│  ├─ available < bytes_needed → Score -= 50                  │
│  │                             reason = InsufficientCapacity│
│  └─ Sufficient → Continue                                   │
│                                                             │
│  Check 3: Price                                             │
│  ├─ price > max_price → Score -= 30                         │
│  │                      reason = PriceTooHigh               │
│  └─ Within budget → Continue                                │
│                                                             │
│  Check 4: Duration                                          │
│  ├─ duration not in range → Score -= 20                     │
│  │                          reason = DurationMismatch       │
│  └─ Duration OK → Continue                                  │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│  Final Score: 0-100                                         │
│  - 100: Perfect match                                       │
│  - 70-99: Good match with minor issues                      │
│  - 50-69: Partial match                                     │
│  - <50: Poor match                                          │
└─────────────────────────────────────────────────────────────┘
```

### 5. Match Result Sorting

Results are sorted for optimal selection:

1. **Primary sort**: Match score (descending)
2. **Secondary sort**: Price per byte (ascending)

This ensures users see the best matches first, with cheaper options preferred among equal scores.

## API Reference

### Runtime API

#### find_matching_providers

Find providers matching storage requirements.

```rust
fn find_matching_providers(
    requirements: StorageRequirements,
    limit: u32
) -> Vec<MatchedProvider>;
```

**Parameters:**
- `requirements`: Storage criteria to match
- `limit`: Maximum providers to return

**Returns:** List of matched providers sorted by score

#### providers_with_capacity

Get providers with sufficient capacity (paginated).

```rust
fn providers_with_capacity(
    bytes_needed: u64,
    offset: u32,
    limit: u32
) -> Vec<(AccountId, ProviderInfoResponse)>;
```

**Parameters:**
- `bytes_needed`: Required storage capacity
- `offset`: Pagination offset
- `limit`: Maximum results

**Returns:** List of provider accounts with their info

### Client SDK

```rust
use storage_client::{DiscoveryClient, StorageRequirements};

// Create client
let mut client = DiscoveryClient::with_defaults()?;
client.connect().await?;

// Define requirements
let requirements = StorageRequirements {
    bytes_needed: 10 * 1024 * 1024 * 1024, // 10 GB
    min_duration: 100_000,                  // ~2 weeks
    max_price_per_byte: 1_000_000,          // Budget limit
    primary_only: true,
};

// Find matching providers
let providers = client.find_providers(requirements, 10).await?;

// Or get the best match directly
let best = client.find_best_provider(requirements).await?;

// Or get recommendations with cost estimates
let recommendations = client.suggest_providers(
    10 * 1024 * 1024 * 1024, // bytes
    100_000,                  // duration
    1_000_000_000_000,        // budget
).await?;
```

## Data Flow

### Provider Registration Flow

```
Provider                    Chain                      Storage
   │                          │                          │
   │  register_provider()     │                          │
   │  + stake                 │                          │
   │─────────────────────────▶│                          │
   │                          │                          │
   │                          │  Store ProviderInfo      │
   │                          │─────────────────────────▶│
   │                          │  (max_capacity = 0)      │
   │                          │                          │
   │  update_provider_settings│                          │
   │  (max_capacity = 1TB)    │                          │
   │─────────────────────────▶│                          │
   │                          │                          │
   │                          │  Validate:               │
   │                          │  - capacity >= committed │
   │                          │  - stake covers capacity │
   │                          │                          │
   │                          │  Update ProviderInfo     │
   │                          │─────────────────────────▶│
   │                          │                          │
```

### Provider Discovery Flow

```
User                       Client                     Chain
  │                          │                          │
  │  "Need 100GB storage"    │                          │
  │─────────────────────────▶│                          │
  │                          │                          │
  │                          │  find_matching_providers │
  │                          │─────────────────────────▶│
  │                          │                          │
  │                          │                          │
  │                          │  For each provider:      │
  │                          │  - Check accepting       │
  │                          │  - Check capacity        │
  │                          │  - Check price           │
  │                          │  - Check duration        │
  │                          │  - Calculate score       │
  │                          │                          │
  │                          │◀─────────────────────────│
  │                          │  Sorted results          │
  │                          │                          │
  │  Display recommendations │                          │
  │◀─────────────────────────│                          │
  │                          │                          │
  │  Select provider         │                          │
  │─────────────────────────▶│                          │
  │                          │                          │
  │                          │  request_agreement()     │
  │                          │─────────────────────────▶│
  │                          │                          │
```

## Economic Model

### Stake Requirements

Providers must stake tokens proportional to their declared capacity:

```
required_stake = max_capacity * MinStakePerByte
```

**Example:**
- `MinStakePerByte = 1_000_000` (1 unit per MB)
- Provider wants `max_capacity = 1TB`
- Required stake = 1TB × 1_000_000 = 1,000,000,000,000 units

### Incentive Alignment

| Actor | Incentive | Mechanism |
|-------|-----------|-----------|
| Provider | Maximize utilization | Higher `committed_bytes/max_capacity` = more revenue |
| Provider | Accurate capacity | Over-commitment → slashing risk |
| User | Find best value | Matching algorithm optimizes price/quality |
| User | Provider reliability | Match score includes challenge history |

### Capacity Utilization

```
Available Capacity = max_capacity - committed_bytes

Utilization Rate = committed_bytes / max_capacity × 100%

Example:
- max_capacity: 1 TB
- committed_bytes: 750 GB
- available: 250 GB
- utilization: 75%
```

## Error Handling

### Capacity-Related Errors

| Error | Cause | Resolution |
|-------|-------|------------|
| `CapacityBelowCommitted` | Setting `max_capacity` below current commitments | Wait for agreements to end or set higher capacity |
| `CapacityExceeded` | Accepting agreement that would exceed capacity | Reject agreement or increase capacity first |
| `InsufficientStakeForCapacity` | Not enough stake to back declared capacity | Add more stake or reduce capacity |

### Matching Failures

| Partial Match Reason | Meaning |
|---------------------|---------|
| `PriceTooHigh` | Provider's price exceeds user's budget |
| `InsufficientCapacity` | Provider can't accommodate requested bytes |
| `DurationMismatch` | Agreement duration outside provider's range |
| `NotAccepting` | Provider not accepting new agreements |

## Future Enhancements (Phase 2+)

### Commitment Periods
- Providers commit to availability periods
- Early exit penalties
- Auto-renewal options

### Automatic Data Migration
- When provider exits, data migrates to alternatives
- Migration cost sharing
- Graceful degradation

### Reputation System
- Historical performance scoring
- Challenge success rates
- Uptime tracking

### Geographic Matching
- Multiaddr parsing for location
- Latency-based matching
- Redundancy across regions

## Configuration

### Runtime Parameters

```rust
// Minimum stake per byte of declared capacity
pub const MinStakePerByte: Balance = 1_000_000;

// Minimum provider stake to register
pub const MinProviderStake: Balance = 1_000 * UNIT;

// Challenge response timeout
pub const ChallengeTimeout: BlockNumber = 100;
```

### Client Configuration

```rust
let config = ClientConfig {
    chain_rpc: "ws://127.0.0.1:2222".to_string(),
    provider_http: "http://localhost:3333".to_string(),
    chunk_size: 256 * 1024,
    max_retries: 3,
};
```

## Security Considerations

### Sybil Resistance
- Stake requirements prevent cheap identity creation
- Capacity backed by economic commitment

### Capacity Gaming
- Cannot over-report capacity (stake limits)
- Cannot under-report (loses business)
- Market forces drive honest reporting

### Price Manipulation
- Transparent on-chain pricing
- Users can compare across providers
- Competition drives fair prices
