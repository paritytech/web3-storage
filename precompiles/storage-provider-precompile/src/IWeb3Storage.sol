// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.0;

/// @title IWeb3Storage
/// @notice Solidity interface for the web3-storage `pallet_storage_provider`
///         precompile (client-side bucket lifecycle). Substrate `AccountId32`
///         values (32-byte sr25519 public keys) cross the boundary as `bytes32`;
///         the EVM caller's substrate-mapped account is derived from
///         `msg.sender` via `AccountId32Mapper`.
///
/// Role tags: 0 = Admin, 1 = Writer, 2 = Reader.
interface IWeb3Storage {
    // --- Bucket lifecycle ---------------------------------------------------

    /// Create an empty bucket. The caller (substrate-mapped) becomes the bucket
    /// admin. Returns the new bucket id.
    function createBucket(uint32 minProviders) external returns (uint64 bucketId);

    /// Create a bucket and atomically open a primary agreement against an
    /// auto-matched provider. The caller's reserved balance must cover the
    /// payment derived from `maxBytes * duration * matched-price`.
    function createBucketWithStorage(
        uint64 maxBytes,
        uint32 duration,
        uint128 maxPricePerByte
    ) external returns (uint64 bucketId);

    /// Freeze a bucket — append-only, irreversible.
    function freezeBucket(uint64 bucketId) external;

    // --- Membership ---------------------------------------------------------

    /// Add or update a bucket member.
    function setMember(uint64 bucketId, bytes32 member, uint8 role) external;

    /// Remove a member from a bucket.
    function removeMember(uint64 bucketId, bytes32 member) external;

    // --- Agreement lifecycle ------------------------------------------------

    /// Request a primary storage agreement from `provider`. Provider must
    /// `accept` separately (substrate side); the caller is bucket admin.
    function requestPrimaryAgreement(
        uint64 bucketId,
        bytes32 provider,
        uint64 maxBytes,
        uint32 duration,
        uint128 maxPayment
    ) external;

    /// Add funds and capacity to an existing agreement.
    function topUpAgreement(
        uint64 bucketId,
        bytes32 provider,
        uint64 additionalBytes,
        uint128 maxPayment
    ) external;

    /// Extend an existing agreement's duration.
    function extendAgreement(
        uint64 bucketId,
        bytes32 provider,
        uint32 additionalDuration,
        uint128 maxPayment
    ) external;

    /// End an agreement, paying the provider in full.
    function endAgreementPay(uint64 bucketId, bytes32 provider) external;

    /// End an agreement, burning `burnPercent` (0-100) and paying the rest.
    function endAgreementBurn(uint64 bucketId, bytes32 provider, uint8 burnPercent) external;

    // --- Challenges ---------------------------------------------------------

    /// Challenge a provider's checkpoint at a specific leaf/chunk.
    function challengeCheckpoint(
        uint64 bucketId,
        bytes32 provider,
        uint64 leafIndex,
        uint64 chunkIndex
    ) external;
}
