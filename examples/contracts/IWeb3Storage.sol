// SPDX-License-Identifier: Apache-2.0

pragma solidity ^0.8.34;

/// @title IWeb3Storage
/// @notice Solidity interface for the web3-storage `pallet_storage_provider`
///         precompile (client-side bucket lifecycle). Substrate `AccountId32`
///         values (32-byte sr25519 public keys) cross the boundary as `bytes32`;
///         the EVM caller's substrate-mapped account is derived from
///         `msg.sender` via `AccountId32Mapper`.
interface IWeb3Storage {
    /// Bucket member role.
    enum Role {
        Admin,
        Writer,
        Reader
    }

    /// Bucket read visibility.
    enum Visibility {
        Public,
        Private
    }

    // TODO: Find out way to make it re-useable
    struct PrimitiveReplicaTerms {
        /// Balance reserved by the owner to fund per-sync confirmations.
        uint128 syncBalance;
        /// Minimum blocks between sync confirmations the provider commits to.
        uint32 minSyncInterval;
        /// Price per sync locked at creation/last extension
        uint128 syncPrice;
    }

    struct PrimitiveAgreementTerms {
        /// Owner bound by these terms (must be the caller's substrate-mapped
        /// account at redemption).
        bytes32 owner;
        /// Storage quota committed by the provider, in bytes.
        uint64 maxBytes;
        /// Agreement duration in blocks from activation.
        uint32 duration;
        /// Price per byte per block locked at quote time.
        uint128 pricePerByte;
        /// Block number after which the quote is no longer redeemable.
        uint32 validUntil;
        /// Provider-chosen replay-protection nonce.
        uint64 nonce;
        /// `true` if the quote is bound to an existing bucket (`Some(_)` on
        /// the Rust side) — required for replica terms; primary terms leave
        /// this false.
        bool hasBucketId;
        /// Target bucket id; only read when `hasBucketId` is true.
        uint64 bucketId;
        /// `true` if the provider quoted replica terms (`Some(_)` on the Rust side).
        bool hasReplicaParams;
        /// Replica funding parameters; only read when `hasReplicaParams` is true.
        PrimitiveReplicaTerms replicaParams;
    }

    // --- Bucket lifecycle ---------------------------------------------------

    /// Redeem provider-signed agreement terms: create a bucket and open a
    /// primary agreement atomically. `terms` must match the SCALE payload the
    /// provider signed; `terms.owner` must be the caller's substrate-mapped
    /// account. `signature` is the SCALE-encoded `MultiSignature` from the
    /// provider's `/negotiate` response (variant byte + raw signature bytes).
    /// `visibility` sets the new bucket's read visibility. Returns the new
    /// bucket id.
    function establishStorageAgreement(
        bytes32 provider,
        PrimitiveAgreementTerms calldata terms,
        bytes calldata signature,
        Visibility visibility
    ) external returns (uint64 bucketId);

    /// Freeze a bucket — append-only, irreversible.
    function freezeBucket(uint64 bucketId) external;

    /// Set bucket read visibility (admin only).
    /// Always reversible on-chain, but publicizing cannot recall data
    /// already disclosed.
    function setBucketVisibility(uint64 bucketId, Visibility visibility) external;

    // --- Membership ---------------------------------------------------------

    /// Add or update a bucket member.
    function setMember(uint64 bucketId, bytes32 member, Role role) external;

    /// Remove a member from a bucket.
    function removeMember(uint64 bucketId, bytes32 member) external;

    // --- Agreement lifecycle ------------------------------------------------

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
