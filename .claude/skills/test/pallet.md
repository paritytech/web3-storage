---
name: test-pallet
description: Run comprehensive tests for the storage pallet
argument-hint: "[--benchmarks] [--coverage]"
disable-model-invocation: true
user-invocable: true
allowed-tools: Bash, Read, Edit, Glob, Grep
---

# Test Storage Pallet $ARGUMENTS

Run comprehensive tests for the Scalable Web3 Storage pallet.

## Arguments

- `--benchmarks` - Also run benchmark tests
- `--coverage` - Generate test coverage report

## Step 1: Pre-flight Checks

```bash
git status --porcelain
cargo --version
rustc --version
```

## Step 2: Format and Lint

```bash
# Format check
cargo fmt --all -- --check

# Clippy with warnings as errors
cargo clippy --all-targets --all-features --workspace -- -D warnings
```

**If any fail:** Fix formatting/linting issues before proceeding.

## Step 3: Run Unit Tests

```bash
# Pallet tests
cargo test -p storage-provider-pallet --all-features

# Primitives tests
cargo test -p storage-primitives

# Runtime tests
cargo test -p storage-parachain-runtime
```

## Step 4: Run Integration Tests

```bash
# Provider node tests
cargo test -p storage-provider-node

# Client SDK tests
cargo test -p storage-client --all-features
```

## Step 5: Run Benchmarks (if --benchmarks flag)

```bash
# Build runtime with benchmark features
cargo build --release -p storage-parachain-runtime --features runtime-benchmarks

# Run pallet benchmarks
cargo test -p storage-provider-pallet --features runtime-benchmarks benchmark
```

## Step 6: Coverage Report (if --coverage flag)

```bash
# Install tarpaulin if not present
cargo install cargo-tarpaulin

# Generate coverage
cargo tarpaulin --workspace --out Html --output-dir target/coverage
```

**Report location:** `target/coverage/index.html`

## Step 7: Test Report

Generate summary:

```
✅ Pallet Tests Complete

Unit Tests:
- Pallet: [PASS/FAIL]
- Primitives: [PASS/FAIL]
- Runtime: [PASS/FAIL]

Integration Tests:
- Provider Node: [PASS/FAIL]
- Client SDK: [PASS/FAIL]

Benchmarks: [PASS/FAIL/SKIPPED]
Coverage: [X%/SKIPPED]
```

## Common Test Patterns

### Testing Extrinsics
```rust
#[test]
fn test_register_provider() {
    new_test_ext().execute_with(|| {
        // Setup
        let provider = account(0);
        let stake = 1000 * UNIT;

        // Execute
        assert_ok!(StorageProvider::register_provider(
            RuntimeOrigin::signed(provider),
            multiaddr,
            public_key,
            stake
        ));

        // Verify
        assert!(Providers::<Test>::contains_key(provider));
    });
}
```

### Testing Error Cases
```rust
#[test]
fn test_insufficient_stake() {
    new_test_ext().execute_with(|| {
        let stake = 100; // Too low
        assert_noop!(
            StorageProvider::register_provider(/*...*/),
            Error::<Test>::InsufficientStake
        );
    });
}
```

### Testing Events
```rust
#[test]
fn test_provider_registered_event() {
    new_test_ext().execute_with(|| {
        // Execute
        assert_ok!(StorageProvider::register_provider(/*...*/));

        // Verify event
        System::assert_last_event(
            Event::ProviderRegistered { provider, stake }.into()
        );
    });
}
```

## Critical Test Areas

1. **Provider Registration**
   - Stake requirements
   - Duplicate registration
   - Invalid parameters

2. **Storage Agreements**
   - Payment calculation
   - Duration limits
   - Capacity limits
   - Agreement acceptance/rejection

3. **Checkpoints**
   - MMR root verification
   - Signature validation
   - Threshold requirements

4. **Challenges**
   - Proof verification
   - Slashing logic
   - Challenge timeouts

5. **Bucket Management**
   - Creation
   - Member management
   - Freezing/unfreezing

## Links

- [Pallet Source](/pallet/src/lib.rs)
- [Testing Guide](docs/testing/MANUAL_TESTING_GUIDE.md)
- [Runtime](/runtime/src/lib.rs)
