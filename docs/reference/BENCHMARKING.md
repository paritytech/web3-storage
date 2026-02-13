# Benchmarking Guide

This guide covers how to benchmark the storage provider pallet to generate accurate weight values for extrinsics.

## Overview

FRAME benchmarking allows measuring the actual execution time and resource usage of pallet extrinsics. This data is used to calculate weights, which are crucial for:

- **Transaction fees**: Users pay fees proportional to computation used
- **Block limits**: Prevents blocks from taking too long to execute
- **DoS protection**: Ensures attackers can't submit cheap but expensive operations

## Architecture

### Files Structure

```
pallet/src/
├── lib.rs           # Main pallet with WeightInfo trait bound
├── weights.rs       # WeightInfo trait + implementations
├── benchmarking.rs  # Benchmark functions (only with runtime-benchmarks feature)
└── mock.rs          # Test runtime with WeightInfo = ()
```

### WeightInfo Trait

The `WeightInfo` trait in `weights.rs` defines weight functions for all extrinsics:

```rust
pub trait WeightInfo {
    fn register_provider() -> Weight;
    fn deregister_provider() -> Weight;
    fn create_bucket() -> Weight;
    // ... 33 more functions
}
```

### Implementations

1. **`SubstrateWeight<T>`** - Production weights with DB read/write costs
2. **`impl WeightInfo for ()`** - Minimal weights for testing

## Building with Benchmarks

```bash
# Build the pallet with benchmarking support
cargo build -p pallet-storage-provider --features runtime-benchmarks

# Build release with benchmarks (recommended for actual measurements)
cargo build --release --features runtime-benchmarks
```

## Running Benchmarks

### Full Benchmark Run

```bash
./target/release/parachain-node benchmark pallet \
    --chain dev \
    --pallet pallet_storage_provider \
    --extrinsic "*" \
    --steps 50 \
    --repeat 20 \
    --output pallet/src/weights.rs
```

### Options Explained

| Option | Description |
|--------|-------------|
| `--chain dev` | Use development chain spec |
| `--pallet pallet_storage_provider` | Target our pallet |
| `--extrinsic "*"` | Benchmark all extrinsics |
| `--steps 50` | Number of steps for linear components |
| `--repeat 20` | Repeat each benchmark 20 times |
| `--output` | Write generated weights to file |

### Benchmark Specific Extrinsics

```bash
# Only benchmark checkpoint-related extrinsics
./target/release/parachain-node benchmark pallet \
    --chain dev \
    --pallet pallet_storage_provider \
    --extrinsic "checkpoint" \
    --extrinsic "provider_checkpoint" \
    --steps 50 \
    --repeat 20 \
    --output pallet/src/weights.rs
```

## Extrinsics Covered

### Provider Management (5)
| Extrinsic | Weight Function | Description |
|-----------|-----------------|-------------|
| `register_provider` | `register_provider()` | Register as storage provider |
| `deregister_provider` | `deregister_provider()` | Remove provider registration |
| `update_provider_settings` | `update_provider_settings()` | Update provider configuration |
| `add_stake` | `add_stake()` | Add stake to provider |
| `set_extensions_blocked` | `block_extensions()` | Block/unblock agreement extensions |

### Bucket Management (6)
| Extrinsic | Weight Function | Description |
|-----------|-----------------|-------------|
| `create_bucket` | `create_bucket()` | Create new bucket |
| `set_min_providers` | `set_bucket_min_providers()` | Set minimum provider threshold |
| `freeze_bucket` | `freeze_bucket()` | Make bucket append-only |
| `set_member` | `set_bucket_member()` | Add/update bucket member |
| `remove_member` | `remove_bucket_member()` | Remove bucket member |
| `remove_slashed` | `remove_slashed()` | Clean up slashed provider |

### Agreement Management (9)
| Extrinsic | Weight Function | Description |
|-----------|-----------------|-------------|
| `request_agreement` | `request_agreement()` | Request replica agreement |
| `request_primary_agreement` | `request_primary_agreement()` | Request primary agreement |
| `accept_agreement` | `accept_agreement()` | Accept pending agreement |
| `reject_agreement` | `reject_agreement()` | Reject pending agreement |
| `withdraw_agreement_request` | `withdraw_agreement_request()` | Withdraw request |
| `top_up_agreement` | `top_up_agreement()` | Increase agreement bytes |
| `extend_agreement` | `extend_agreement()` | Extend agreement duration |
| `end_agreement` | `end_agreement()` | End with pay/burn |
| `claim_expired_agreement` | `claim_expired_agreement()` | Provider claims after expiry |

### Checkpoints (3)
| Extrinsic | Weight Function | Description |
|-----------|-----------------|-------------|
| `checkpoint` | `checkpoint()` | Submit client checkpoint |
| `extend_checkpoint` | `extend_checkpoint()` | Add signatures to checkpoint |
| `fund_checkpoint_pool` | `fund_checkpoint_pool()` | Fund reward pool |

### Provider-Initiated Checkpoints (4)
| Extrinsic | Weight Function | Description |
|-----------|-----------------|-------------|
| `provider_checkpoint` | `provider_checkpoint(s)` | Provider submits checkpoint (s = signature count) |
| `configure_checkpoint_window` | `configure_checkpoint_window()` | Configure checkpoint settings |
| `report_missed_checkpoint` | `report_missed_checkpoint()` | Report missed window |
| `claim_checkpoint_rewards` | `claim_checkpoint_rewards()` | Claim accumulated rewards |

### Challenge System (4)
| Extrinsic | Weight Function | Description |
|-----------|-----------------|-------------|
| `challenge_checkpoint` | `challenge_checkpoint()` | Challenge on-chain commitment |
| `challenge_offchain` | `challenge_off_chain()` | Challenge with signature |
| `challenge_replica` | `challenge_replica()` | Challenge replica provider |
| `respond_to_challenge` | `respond_to_challenge()` | Provider responds to challenge |

### Replica Sync (2)
| Extrinsic | Weight Function | Description |
|-----------|-----------------|-------------|
| `confirm_replica_sync` | `confirm_replica_sync()` | Replica confirms sync |
| `top_up_replica_sync_balance` | `top_up_replica_sync_balance()` | Top up sync balance |

## Parameterized Benchmarks

Some extrinsics have variable costs based on input size. These use `Linear` parameters:

```rust
#[benchmark]
fn provider_checkpoint(s: Linear<1, 5>) {
    // s = number of signatures
    // Weight scales with signature verification count
}
```

The generated weight function accepts the parameter:

```rust
fn provider_checkpoint(s: u32) -> Weight {
    Weight::from_parts(70_000_000, 7000)
        .saturating_add(Weight::from_parts(15_000_000, 0).saturating_mul(s.into()))
        .saturating_add(T::DbWeight::get().reads(5_u64))
        .saturating_add(T::DbWeight::get().writes(4_u64))
}
```

## Understanding Weight Output

Generated weights include:

```rust
fn register_provider() -> Weight {
    Weight::from_parts(50_000_000, 5000)      // (ref_time, proof_size)
        .saturating_add(T::DbWeight::get().reads(2_u64))   // DB reads
        .saturating_add(T::DbWeight::get().writes(2_u64))  // DB writes
}
```

| Component | Description |
|-----------|-------------|
| `ref_time` | Reference time in picoseconds |
| `proof_size` | Proof size for parachain validation |
| `reads` | Number of storage reads |
| `writes` | Number of storage writes |

## Current Estimated Weights

These are estimated weights before running actual benchmarks:

| Category | Extrinsic | Ref Time | Proof Size | Reads | Writes |
|----------|-----------|----------|------------|-------|--------|
| Provider | `register_provider` | 50ms | 5KB | 2 | 2 |
| Provider | `deregister_provider` | 40ms | 4KB | 2 | 2 |
| Bucket | `create_bucket` | 45ms | 4.5KB | 1 | 2 |
| Agreement | `accept_agreement` | 60ms | 6KB | 4 | 4 |
| Checkpoint | `checkpoint` | 80ms | 8KB | 4 | 2 |
| Checkpoint | `provider_checkpoint` | 70ms + 15ms×s | 7KB | 5 | 4 |
| Challenge | `respond_to_challenge` | 150ms | 15KB | 5 | 4 |

## Adding Benchmarks for New Extrinsics

When adding a new extrinsic:

1. **Add to WeightInfo trait** (`weights.rs`):
   ```rust
   fn new_extrinsic() -> Weight;
   ```

2. **Add SubstrateWeight implementation** (`weights.rs`):
   ```rust
   fn new_extrinsic() -> Weight {
       Weight::from_parts(30_000_000, 3000)
           .saturating_add(T::DbWeight::get().reads(2_u64))
           .saturating_add(T::DbWeight::get().writes(1_u64))
   }
   ```

3. **Add () implementation** (`weights.rs`):
   ```rust
   fn new_extrinsic() -> Weight { Weight::from_parts(10_000, 0) }
   ```

4. **Add benchmark function** (`benchmarking.rs`):
   ```rust
   #[benchmark]
   fn new_extrinsic() {
       // Setup
       let caller = funded_account::<T>("caller", 0);

       #[extrinsic_call]
       new_extrinsic(RawOrigin::Signed(caller), param1, param2);
   }
   ```

5. **Use in extrinsic** (`lib.rs`):
   ```rust
   #[pallet::weight(T::WeightInfo::new_extrinsic())]
   pub fn new_extrinsic(...) -> DispatchResult { ... }
   ```

## Testing Benchmarks

Run benchmark tests without actually measuring:

```bash
cargo test -p pallet-storage-provider --features runtime-benchmarks
```

This runs the benchmark setup and verification without timing.

## Best Practices

1. **Run benchmarks on reference hardware**: Use similar specs to production validators
2. **Use release builds**: Debug builds have different performance characteristics
3. **Repeat multiple times**: Use `--repeat 20` or more for stable measurements
4. **Benchmark after significant changes**: Re-run benchmarks when logic changes
5. **Include worst-case scenarios**: Benchmarks should test maximum complexity
6. **Test with realistic data**: Use representative account states and storage

## Troubleshooting

### "Benchmark failed"

Some benchmarks may fail if prerequisites aren't met (e.g., no agreement exists). This is expected for certain edge cases. The benchmark measures the cost of validation checks.

### "Not enough balance"

Increase the funded amount in `funded_account`:
```rust
let amount = BalanceOf::<T>::max_value() / 2u32.into();
```

### "Feature not enabled"

Ensure you're building with `--features runtime-benchmarks`.

## Related Documentation

- [Extrinsics Reference](./EXTRINSICS_REFERENCE.md) - Complete API documentation
- [Substrate Benchmarking](https://docs.substrate.io/test/benchmark/) - Official Substrate docs
- [FRAME Benchmarking](https://paritytech.github.io/polkadot-sdk/master/frame_benchmarking/) - API reference
