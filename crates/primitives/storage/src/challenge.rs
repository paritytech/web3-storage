// SPDX-License-Identifier: Apache-2.0

//! Challenge identifiers and per-challenger statistics.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Aggregated per-challenger statistics kept on-chain so the SDK can answer
/// "how many challenges have I issued / won / lost / earned" without scanning
/// historical events. Updated on `create_challenge`, on `ChallengeDefended`,
/// and on `ChallengeSlashed`.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengerStatRecord {
    /// Total challenges the challenger has ever opened.
    pub total_challenges: u32,
    /// Challenges where the provider was slashed (either invalid response or
    /// timeout). The challenger is only made whole (deposit refunded) and earns
    /// no reward — the slashed stake goes entirely to the Treasury, per the
    /// design's challenge model.
    pub successful_challenges: u32,
    /// Challenges where the provider successfully defended.
    pub failed_challenges: u32,
}

/// Challenge identifier combining deadline and index.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengeId<BlockNumber> {
    /// Block by which provider must respond
    pub deadline: BlockNumber,
    /// Index within the deadline's challenge list
    pub index: u16,
}
