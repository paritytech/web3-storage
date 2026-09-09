// SPDX-License-Identifier: Apache-2.0

pragma solidity ^0.8.34;

/// @title IDriveRegistry
/// @notice Solidity interface for the web3-storage `pallet_drive_registry`
///         precompile. Substrate `AccountId32` values cross the boundary as
///         `bytes32`; the EVM caller's substrate-mapped account becomes the
///         drive owner.
interface IDriveRegistry {
    /// Drive member role.
//
// Vendored verbatim from `examples/contracts/IDriveRegistry.sol` so the Photos
// app is self-contained. Keep in sync if the canonical interface changes.
    enum Role {
        Admin,
        Writer,
        Reader
    }

    /// Read visibility of the drive's underlying Layer 0 bucket.
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

    /// Create a new drive by redeeming provider-signed agreement terms: the
    /// underlying Layer 0 bucket and primary agreement are opened atomically.
    ///
    /// - `name` may be empty (treated as `None`).
    /// - `terms` must match the SCALE payload the provider signed;
    ///   `terms.owner` must be the caller's substrate-mapped account.
    /// - `signature` is the SCALE-encoded `MultiSignature` from the provider's
    ///   `/negotiate` response (variant byte + raw signature bytes).
    /// - `visibility` sets the underlying Layer 0 bucket's read visibility.
    ///
    /// Returns the new drive id.
    function createDrive(
        string calldata name,
        bytes32 provider,
        PrimitiveAgreementTerms calldata terms,
        bytes calldata signature,
        Visibility visibility
    ) external returns (uint64 driveId);

    /// Delete a drive, refunding any remaining payment to the owner.
    function deleteDrive(uint64 driveId) external;

    /// Share a drive with another account.
    function shareDrive(uint64 driveId, bytes32 member, Role role) external;

    /// Remove a previously-shared member from a drive.
    function unshareDrive(uint64 driveId, bytes32 member) external;
}
