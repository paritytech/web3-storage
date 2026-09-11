# File System Interface - Admin Guide

## Table of Contents

1. [Introduction](#introduction)
2. [Admin Responsibilities](#admin-responsibilities)
3. [System Setup](#system-setup)
4. [Provider Management](#provider-management)
5. [Drive Monitoring](#drive-monitoring)
6. [Policy Configuration](#policy-configuration)
7. [Maintenance Operations](#maintenance-operations)
8. [Dispute Resolution](#dispute-resolution)
9. [System Metrics](#system-metrics)
10. [Troubleshooting](#troubleshooting)

---

## Introduction

As an administrator of the File System Interface, your role is to ensure the system runs smoothly, providers are healthy, and users can reliably store and retrieve their data. Unlike Layer 0 which requires manual intervention for every operation, Layer 1 automates most infrastructure tasks - **you focus on monitoring and policy, not manual setup**.

### Admin Philosophy

**Layer 0 Admin (Old Way):**
- Manual bucket creation for each user
- Manual provider selection
- Manual agreement setup (primary + replicas)
- Manual payment distribution
- Manual failure handling

**Layer 1 Admin (New Way):**
- Monitor system health
- Set policies and defaults
- Ensure provider availability
- Handle escalated issues only

**Result: 250× reduction in admin burden**

---

## Admin Responsibilities

### Primary Responsibilities

1. **Provider Management**
   - ✅ Register and onboard storage providers
   - ✅ Monitor provider health and capacity
   - ✅ Update provider settings and pricing
   - ✅ Handle provider failures (replace/remove)

2. **System Monitoring**
   - ✅ Track total storage usage
   - ✅ Monitor drive creation rate
   - ✅ Watch for capacity issues
   - ✅ Audit checkpoint activity

3. **Policy Configuration**
   - ✅ Set default provider counts
   - ✅ Configure default checkpoint strategies
   - ✅ Define minimum storage requirements
   - ✅ Set pricing guidelines

4. **Dispute Resolution**
   - ✅ Monitor challenges (via Layer 0)
   - ✅ Verify provider commitments
   - ✅ Process slashing events
   - ✅ Replace failed providers

### What You DON'T Do

- ❌ Manually create buckets for users
- ❌ Manually select providers for each drive
- ❌ Manually request storage agreements
- ❌ Distribute payments manually
- ❌ Handle routine operations

The system handles all of this automatically!

---

## System Setup

### Initial Configuration

#### 1. Ensure Runtime Configuration

Check that the runtime has proper configuration in `runtimes/web3-storage-local/src/lib.rs`:

```rust
impl pallet_drive_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type MaxDrivesPerUser = ConstU32<100>;        // Max drives per account
    type MaxDriveNameLength = ConstU32<256>;      // Max name length
}
```

#### 2. Deploy Pallet

Ensure the Drive Registry pallet is included in the runtime:

```rust
construct_runtime!(
    pub enum Runtime {
        System: frame_system,
        Balances: pallet_balances,
        StorageProvider: pallet_storage_provider,  // Layer 0
        DriveRegistry: pallet_drive_registry,       // Layer 1
        // ... other pallets
    }
);
```

#### 3. Verify Genesis State

Check genesis configuration:

```bash
# Verify pallet is initialized
polkadot-js-apps -> Developer -> Chain State -> driveRegistry
```

---

## Provider Management

### Register a Storage Provider

Storage providers must be registered in Layer 0 before they can accept drive agreements:

```rust
// Via Layer 0 pallet
use pallet_storage_provider::Call as StorageProviderCall;

// 1. Provider registers with stake
StorageProviderCall::register_provider {
    multiaddr: b"/ip4/127.0.0.1/tcp/3333".to_vec().try_into().unwrap(),
    // Raw signing key: 32 bytes (Sr25519/Ed25519) or 33 (compressed Ecdsa/Eth)
    public_key: signing_key_bytes.try_into().unwrap(),
    stake: 1_000 * UNIT, // 1000 tokens stake
};

// 2. Admin updates provider settings
StorageProviderCall::update_provider_settings {
    provider: provider_account_id,
    settings: ProviderSettings {
        min_duration: 100,
        max_duration: 100_000,
        price_per_byte: 1_000_000,      // per byte per block
        accepting_primary: true,         // Accept new drives
        replica_sync_price: Some(10_000_000_000),
        accepting_extensions: true,
    },
};
```

### Monitor Provider Health

```rust
// Query all providers
let providers = StorageProvider::query_all_providers();

for (account, info) in providers {
    println!("Provider: {:?}", account);
    println!("  Endpoint: {}", String::from_utf8_lossy(&info.endpoint));
    println!("  Capacity: {} bytes", info.capacity);
    println!("  Used: {} bytes", info.used_capacity);
    println!("  Available: {} bytes", info.capacity - info.used_capacity);
    println!("  Stake: {} tokens", info.stake / UNIT);
    println!("  Status: {:?}", info.status);
    println!("  Accepting: primary={}, extensions={}",
        info.settings.accepting_primary,
        info.settings.accepting_extensions
    );
    println!();
}
```

### Provider Health Checklist

```bash
# 1. HTTP endpoint reachable
curl http://provider.example.com:3000/health
# Expected: {"status":"healthy"}

# 2. Sufficient capacity
# Available capacity should be > 10% of total

# 3. Stake is adequate
# Stake should cover potential slashing

# 4. Provider is accepting agreements
# accepting_primary: true

# 5. No recent slashing events
# Check event logs for provider
```

### Handle Provider Failures

#### Scenario: Provider Goes Offline

```rust
// 1. Detect failure (monitoring system alerts)
// Provider fails health checks for extended period

// 2. Mark provider as unavailable (if needed)
StorageProviderCall::pause_provider {
    provider: failed_provider_id,
};

// 3. System automatically stops routing new drives to this provider

// 4. For existing drives, Layer 0 challenge mechanism handles it:
//    - Challenges are issued
//    - Provider fails to respond
//    - Provider gets slashed
//    - Replica providers take over
```

#### Scenario: Provider Capacity Full

```rust
// Provider capacity exhausted - no admin action needed!
// System automatically:
// 1. Detects provider is at capacity
// 2. Stops routing new drives to this provider
// 3. Selects other providers with available capacity

// Admin can:
// - Add new providers
// - Ask existing provider to increase capacity
// - Monitor and forecast capacity needs
```

#### Replace Failed Provider

```rust
// For drives with failed providers:
// Layer 0 handles this automatically via agreement system

// Admin can monitor:
let failed_agreements = StorageProvider::query_failed_agreements();
println!("Failed agreements: {}", failed_agreements.len());

// If needed, can manually trigger provider replacement:
// (Typically not needed - system handles automatically)
```

---

## Drive Monitoring

### View All Drives

```rust
// Query all drives in the system
let total_drives = DriveRegistry::next_drive_id();
println!("Total drives created: {}", total_drives);

for drive_id in 0..total_drives {
    if let Some(drive_info) = DriveRegistry::get_drive(drive_id) {
        println!("Drive {}: {:?}", drive_id, drive_info.name);
        println!("  Owner: {:?}", drive_info.owner);
        println!("  Bucket: {}", drive_info.bucket_id);
        println!("  Capacity: {} GB", drive_info.max_capacity / 1_000_000_000);
        println!("  Expires: block {}", drive_info.expires_at);
        println!("  Strategy: {:?}", drive_info.commit_strategy);
    }
}
```

### Monitor Storage Usage

```rust
// Calculate total storage allocated
let mut total_allocated = 0u64;
let mut total_drives = 0u64;

for drive_id in 0..DriveRegistry::next_drive_id() {
    if let Some(drive) = DriveRegistry::get_drive(drive_id) {
        total_allocated += drive.max_capacity;
        total_drives += 1;
    }
}

println!("System Statistics:");
println!("  Total Drives: {}", total_drives);
println!("  Total Allocated: {} GB", total_allocated / 1_000_000_000);
println!("  Average per Drive: {} GB",
    (total_allocated / total_drives) / 1_000_000_000
);
```

### Track Drive Activity

```rust
// Monitor recent drive events
// Subscribe to events:
// - DriveCreated
// - RootCIDUpdated
// - DriveDeleted
// - DriveNameUpdated

// Example: Count drives by owner
let mut owner_stats: HashMap<AccountId, u32> = HashMap::new();

for drive_id in 0..DriveRegistry::next_drive_id() {
    if let Some(drive) = DriveRegistry::get_drive(drive_id) {
        *owner_stats.entry(drive.owner).or_insert(0) += 1;
    }
}

println!("Top drive creators:");
for (owner, count) in owner_stats.iter().take(10) {
    println!("  {:?}: {} drives", owner, count);
}
```

### Monitor Checkpoints

```rust
// Track root CID updates (checkpoints)
// Subscribe to RootCIDUpdated events

// Metrics to track:
// - Checkpoint frequency per drive
// - Immediate vs batched vs manual strategy distribution
// - Average time between checkpoints

// Example: Analyze commit strategies
let mut strategy_counts = HashMap::new();

for drive_id in 0..DriveRegistry::next_drive_id() {
    if let Some(drive) = DriveRegistry::get_drive(drive_id) {
        let strategy_key = match drive.commit_strategy {
            CommitStrategy::Immediate => "immediate",
            CommitStrategy::Batched { .. } => "batched",
            CommitStrategy::Manual => "manual",
        };
        *strategy_counts.entry(strategy_key).or_insert(0) += 1;
    }
}

println!("Commit Strategy Distribution:");
for (strategy, count) in strategy_counts {
    println!("  {}: {} drives", strategy, count);
}
```

---

## Policy Configuration

### Default Provider Counts

Current logic (can be customized in pallet):

```rust
// In allocate_bucket_for_user():
let num_providers: u8 = if let Some(min) = min_providers {
    // User-specified
    min
} else {
    // Auto-determine based on storage period
    if storage_period > 1000 {
        3  // Long-term: 1 primary + 2 replicas
    } else {
        1  // Short-term: primary only
    }
};
```

**Customization:**

```rust
// Modify thresholds in pallet code:
// crates/pallets/drive-registry/src/lib.rs

// Example: More aggressive replication
if storage_period > 500 {
    5  // 1 primary + 4 replicas
} else if storage_period > 100 {
    3  // 1 primary + 2 replicas
} else {
    1  // Primary only
}
```

### Default Checkpoint Strategy

```rust
// Current default in primitives:
impl Default for CommitStrategy {
    fn default() -> Self {
        Self::Batched { interval: 100 }  // Every 100 blocks
    }
}

// Customize in file-system-primitives/src/lib.rs:
Self::Batched { interval: 50 }   // More frequent (higher cost)
Self::Batched { interval: 200 }  // Less frequent (lower cost)
```

### Storage Limits

```rust
// Set in runtime configuration:
impl pallet_drive_registry::Config for Runtime {
    // Maximum drives per user
    type MaxDrivesPerUser = ConstU32<100>;  // Increase for power users

    // Maximum drive name length
    type MaxDriveNameLength = ConstU32<256>;  // ASCII characters
}
```

### Pricing Guidelines

Set provider pricing recommendations:

```rust
// Example pricing tiers
pub const PRICING_TIERS: &[(u64, u128)] = &[
    // (blocks, price_per_byte)
    (500, 1_000_000),       // Short-term: 1M per byte per block
    (5_000, 800_000),       // Medium-term: 20% discount
    (50_000, 500_000),      // Long-term: 50% discount
];

// Providers can set their own prices, but admins can provide guidance
```

---

## Maintenance Operations

### System Health Checks

```bash
#!/bin/bash
# health-check.sh - Run periodic health checks

echo "=== File System Interface Health Check ==="
echo

# 1. Check provider availability
echo "1. Provider Status:"
providers=$(query_providers)
active=$(echo "$providers" | grep "accepting_primary: true" | wc -l)
total=$(echo "$providers" | wc -l)
echo "   Active Providers: $active / $total"

# 2. Check capacity
echo "2. Capacity Status:"
total_capacity=$(calculate_total_capacity)
used_capacity=$(calculate_used_capacity)
available=$(($total_capacity - $used_capacity))
usage_pct=$((100 * $used_capacity / $total_capacity))
echo "   Total: ${total_capacity} GB"
echo "   Used: ${used_capacity} GB"
echo "   Available: ${available} GB"
echo "   Usage: ${usage_pct}%"

# 3. Check drive creation rate
echo "3. Drive Activity:"
drives_last_hour=$(count_drives_created_last_hour)
drives_last_day=$(count_drives_created_last_day)
echo "   Created (last hour): $drives_last_hour"
echo "   Created (last day): $drives_last_day"

# 4. Check for errors
echo "4. Recent Errors:"
error_count=$(grep "ERROR" logs/*.log | wc -l)
echo "   Log errors (last hour): $error_count"

# 5. Alert if needed
if [ $active -lt 3 ]; then
    echo "⚠️  WARNING: Low provider count!"
fi

if [ $usage_pct -gt 80 ]; then
    echo "⚠️  WARNING: High capacity usage!"
fi

if [ $error_count -gt 10 ]; then
    echo "⚠️  WARNING: High error rate!"
fi
```

### Database Maintenance

```bash
# Monitor on-chain storage usage
polkadot-js-apps -> Developer -> Chain State -> driveRegistry

# Check storage maps size:
# - Drives: number of entries
# - UserDrives: number of entries
# - BucketToDrive: number of entries
# - NextDriveId: current counter

# Storage pruning happens automatically via Substrate
# No manual intervention needed
```

### Log Management

```bash
# Enable debug logging for troubleshooting
export RUST_LOG="pallet_drive_registry=debug,file_system_client=debug"

# Monitor logs
tail -f /var/log/parachain.log | grep "drive_registry"

# Analyze checkpoint activity
grep "RootCIDUpdated" /var/log/parachain.log | wc -l

# Track drive creation
grep "DriveCreated" /var/log/parachain.log
```

### Backup and Recovery

```bash
# 1. Backup chain state (standard Substrate backup)
polkadot-backup export-state --output chain-state.json

# 2. Backup drive registry specifically
polkadot-js-api --ws ws://127.0.0.1:2222 \
  query.driveRegistry.drives.entries | jq > drives-backup.json

# 3. Recovery
# Standard Substrate chain recovery procedures apply
# Drive metadata is on-chain, file data is in provider storage
```

---

## Dispute Resolution

### Monitor Challenges

Challenges are handled at Layer 0, but admins should monitor:

```rust
// Query recent challenges
let challenges = StorageProvider::query_challenges();

for challenge in challenges {
    println!("Challenge ID: {}", challenge.challenge_id);
    println!("  Bucket: {}", challenge.bucket_id);
    println!("  Provider: {:?}", challenge.provider);
    println!("  Status: {:?}", challenge.status);
    println!("  Issued: block {}", challenge.issued_at);

    // Find associated drive
    if let Some(drive_id) = DriveRegistry::bucket_to_drive(challenge.bucket_id) {
        println!("  Drive: {} ({:?})", drive_id,
            DriveRegistry::get_drive(drive_id).unwrap().name
        );
    }
}
```

### Handle Slashing Events

```rust
// Monitor slashing events
// Subscribe to StorageProvider::ProviderSlashed events

// When provider is slashed:
// 1. System automatically handles it (no admin action)
// 2. Other providers take over (if replicas exist)
// 3. User data remains accessible

// Admin should:
// - Notify affected users (if single provider)
// - Remove consistently failing providers
// - Ensure adequate provider redundancy
```

### Dispute Escalation

```bash
# If user reports data loss:

# 1. Verify drive exists
query_drive <drive_id>

# 2. Check associated bucket
query_bucket <bucket_id>

# 3. Verify provider status
query_provider <provider_id>

# 4. Check recent challenges
query_challenges --bucket <bucket_id>

# 5. Verify data availability
# Attempt download from provider HTTP endpoint
curl http://provider.example.com:3000/node?hash=<cid>

# 6. If data truly lost:
# - Check if slashing occurred
# - Verify user has replicas (if 3+ providers)
# - Facilitate data recovery from replicas
```

---

## System Metrics

### Key Performance Indicators (KPIs)

```rust
// 1. Drive Creation Rate
let drives_per_day = count_drives_created_in_period(blocks_per_day);

// 2. Average Drive Size
let avg_size = total_allocated_capacity / total_drives;

// 3. Provider Utilization
let utilization = (used_capacity / total_capacity) * 100;

// 4. Checkpoint Frequency
let checkpoints_per_day = count_root_cid_updates_in_period(blocks_per_day);

// 5. System Uptime
// Track via parachain block production

// 6. Provider Availability
let provider_uptime = healthy_providers / total_providers;
```

### Dashboards

Create monitoring dashboards tracking:

- **Capacity**: Total, used, available, growth rate
- **Activity**: Drives created, files uploaded, checkpoints committed
- **Providers**: Count, capacity, health status, slashing events
- **Performance**: Average response time, error rate, success rate
- **Economics**: Total value locked, payments distributed, slashing amounts

### Alerting Rules

```yaml
# Example alerting configuration

alerts:
  - name: low_provider_count
    condition: active_providers < 3
    severity: critical
    message: "Critical: Less than 3 active providers!"

  - name: high_capacity_usage
    condition: capacity_usage > 80%
    severity: warning
    message: "Warning: System capacity above 80%"

  - name: provider_slashed
    condition: slashing_event_occurred
    severity: high
    message: "Alert: Provider slashed - investigate"

  - name: high_error_rate
    condition: error_rate > 5%
    severity: medium
    message: "Increased error rate detected"
```

---

## Troubleshooting

### Common Admin Issues

#### Issue: "NoProvidersAvailable" Error for Users

**Diagnosis:**
```rust
// Check active providers
let active = StorageProvider::query_available_providers(
    user_capacity,
    true,  // accepting_primary
);

println!("Active providers: {}", active.len());
```

**Solutions:**
- Ensure providers are registered and active
- Verify providers have `accepting_primary: true`
- Check providers have sufficient capacity
- Add new providers if needed

#### Issue: High Capacity Usage

**Diagnosis:**
```bash
# Check per-provider capacity
for provider in $(list_providers); do
    capacity=$(query_provider_capacity $provider)
    used=$(query_provider_used $provider)
    pct=$((100 * $used / $capacity))
    echo "Provider $provider: ${pct}% used"
done
```

**Solutions:**
- Add new providers
- Ask existing providers to increase capacity
- Implement data retention policies

#### Issue: Checkpoint Flooding

**Problem:** Too many checkpoint transactions

**Diagnosis:**
```rust
// Count immediate commit drives
let immediate_count = drives.iter()
    .filter(|d| matches!(d.commit_strategy, CommitStrategy::Immediate))
    .count();

println!("Drives with immediate commits: {}", immediate_count);
```

**Solutions:**
- Educate users about commit strategy costs
- Adjust default to less frequent batching
- Implement rate limiting if needed

#### Issue: Drive Creation Failures

**Diagnosis:**
```bash
# Check recent failed transactions
grep "DriveCreationFailed" parachain.log

# Common failures:
# - InsufficientPayment
# - NoProvidersAvailable
# - InvalidStorageSize
# - InvalidStoragePeriod
```

**Solutions:**
- Verify user has sufficient balance
- Check provider availability
- Validate user input parameters

### Admin Debug Commands

```bash
# List all drives
polkadot-js-api query.driveRegistry.drives.entries

# List drives by owner
polkadot-js-api query.driveRegistry.userDrives <account_id>

# Get drive details
polkadot-js-api query.driveRegistry.drives <drive_id>

# Check next drive ID
polkadot-js-api query.driveRegistry.nextDriveId

# Query bucket-to-drive mapping
polkadot-js-api query.driveRegistry.bucketToDrive <bucket_id>

# List all providers
polkadot-js-api query.storageProvider.providers.entries

# Check provider settings
polkadot-js-api query.storageProvider.providers <account_id>
```

---

## Best Practices

### Provider Management

1. **Maintain Redundancy**: Keep at least 5 active providers
2. **Monitor Capacity**: Keep utilization below 75%
3. **Geographic Distribution**: Encourage providers in different regions
4. **Regular Health Checks**: Automated monitoring every hour
5. **Stake Requirements**: Ensure providers have adequate stake

### System Configuration

1. **Conservative Defaults**: Use safe default values
2. **Document Changes**: Log all configuration changes
3. **Test Before Deploy**: Test policy changes on testnet
4. **Monitor Impact**: Track metrics after changes
5. **Gradual Rollout**: Phase major changes

### Monitoring Strategy

1. **Real-Time Alerts**: Critical issues immediately
2. **Daily Reports**: Capacity, activity, health
3. **Weekly Reviews**: Trends, planning, optimization
4. **Monthly Analysis**: Growth, economics, forecasting

---

## Technical Reference

### Data Encoding

Understanding the encoding system helps with debugging:

**SCALE Encoding**: All data is encoded using Substrate's SCALE codec:
- Deterministic: Same data always produces same bytes
- Used for CID computation and on-chain storage
- See [Architecture Document](./ARCHITECTURE.md#data-encoding--serialization) for details

**Debug Encoding Issues:**
```bash
# Decode a root CID from hex
echo "e835d9bb4ac2c42bd8895fcfb159903f4ce6de8de863182f4fb87c06a23d18b7" | \
  xxd -r -p | subxt decode DirectoryNode

# Verify CID computation
# CID = blake2_256(SCALE_bytes)
cargo run --example verify_encoding
```

### Provider API Considerations

When troubleshooting provider issues, note these API behaviors:

**Read Endpoint**: Avoid `u64::MAX` as length parameter:
```bash
# BAD: Causes chunk calculation overflow, returns empty
curl "127.0.0.1:3333/read?data_root=0x...&offset=0&length=18446744073709551615"

# GOOD: Use reasonable max (1 TiB)
curl "127.0.0.1:3333/read?data_root=0x...&offset=0&length=1099511627776"
```

**Upload Verification**: Verify uploaded data by checking CID:
```bash
# Upload returns data_root
# Verify: curl /node?hash=<data_root> returns the data
```

---

## Next Steps

- **[User Guide](./USER_GUIDE.md)** - Help users get started
- **[API Reference](./API_REFERENCE.md)** - Complete API documentation
- **[Architecture](./ARCHITECTURE.md)** - System design
- **[Architecture Deep Dive](./ARCHITECTURE.md)** - Encoding, security, blockchain details

## Additional Resources

- **[Layer 0 Admin Guide](../reference/EXTRINSICS_REFERENCE.md)** - Layer 0 operations
- **[Layer 1 Quick Start](../getting-started/LAYER1_QUICKSTART.md)** — three-terminal setup + SDK examples
- **[Design Documents](../design/)** - Architecture specifications
