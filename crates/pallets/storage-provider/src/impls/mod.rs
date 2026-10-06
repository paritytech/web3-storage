// SPDX-License-Identifier: Apache-2.0

/// Agreement lifecycle: pricing, opening from signed terms, settlement.
pub mod agreements;
/// Bucket creation and teardown.
pub mod buckets;
/// Challenge creation, resolution and slashing.
pub mod challenges;
/// Snapshot root history used by checkpoints and replica sync.
pub mod checkpoints;
/// Storage-deposit footprints per record kind.
pub mod deposits;
/// Hold, release and settlement helpers, one per `HoldReason`, each with a
/// fixed `Precision`.
pub mod funds;
/// Provider discovery queries: matching, capacity, challenge candidates.
pub mod marketplace;
/// Bucket membership and role checks.
pub mod members;
/// Provider registration and settings validation.
pub mod providers;
/// Read-only views backing the runtime API.
pub mod queries;
/// Signature verification for commitments and signed terms.
pub mod signatures;
/// Storage invariant checks, run by try-runtime and callable from tests.
pub mod try_state;
