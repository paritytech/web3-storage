// SPDX-License-Identifier: Apache-2.0

//! Random-trace replay of the challenge-protocol spec against the pallet.
//!
//! Requires the `quint` CLI on PATH (`npm i -g @informalsystems/quint`);
//! gated behind the `mbt` feature so plain `cargo test --workspace` skips it:
//!
//! ```sh
//! cargo test -p pallet-storage-provider-mbt --features mbt
//! ```
//!
//! After every step the full chain-visible state (balances, stake, pending
//! counters, snapshot, open challenges) is compared against the spec state;
//! any divergence fails with a diff. `QUINT_SEED=<seed>` reproduces a failed
//! run, `QUINT_VERBOSE=1 cargo test ... -- --nocapture` shows the steps.

use pallet_storage_provider_mbt::ChallengeDriver;
use quint_connect::quint_run;

// 25 steps reaches the tail of the state space (completeDeregP needs the
// agreement to expire plus the announcement window).
#[quint_run(
    spec = "../../../../specs/quint/challenges.qnt",
    main = "challengesCode",
    max_samples = 100,
    max_steps = 25
)]
fn replay_challenges_code() -> impl quint_connect::Driver {
    ChallengeDriver::default()
}
