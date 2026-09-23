// SPDX-License-Identifier: Apache-2.0

//! Reusable storage actions that scenarios compose.
//!
//! Each action owns its `Operation` marker (from [`crate::metrics`]) and a
//! primitive that performs the operation once and returns a measured
//! [`crate::metrics::OpOutcome`]. Scenarios such as `stress-test` drive these
//! primitives under load; a future `read`/`delete` action is a sibling module
//! here, usable by any scenario without touching the metrics layer.

pub mod upload;
