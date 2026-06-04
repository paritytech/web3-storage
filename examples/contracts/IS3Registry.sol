// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.34;

/// @title IS3Registry
/// @notice Solidity interface for the web3-storage `pallet_s3_registry`
///         precompile. The EVM caller's substrate-mapped account becomes the
///         bucket owner; bucket names follow the S3 convention (3-63 chars,
///         lowercase alphanumeric + hyphens). `cid` is a substrate `H256`.
interface IS3Registry {
    // TODO: Find out way to make it re-useable
    struct PrimitiveReplicaTerms {
        /// Balance reserved by the owner to fund per-sync confirmations.
        uint128 syncBalance;
        /// Minimum blocks between sync confirmations the provider commits to.
        uint32 minSyncInterval;
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

    /// Create an S3 bucket by redeeming provider-signed agreement terms: the
    /// underlying Layer 0 bucket and primary agreement are opened atomically.
    /// `terms` must match the SCALE payload the provider signed; `terms.owner`
    /// must be the caller's substrate-mapped account. `signature` is the
    /// SCALE-encoded `MultiSignature` from the provider's `/negotiate`
    /// response (variant byte + raw signature bytes).
    function createS3Bucket(
        string calldata name,
        bytes32 provider,
        PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external returns (uint64 s3BucketId);

    /// Delete an empty bucket. Caller must be the owner.
    function deleteS3Bucket(uint64 s3BucketId) external;

    /// Store / overwrite an object's metadata. `cid` is the off-chain
    /// content hash; `size` is bytes; `contentType` mirrors HTTP. User
    /// metadata is not exposed in v1 (always empty); use the substrate
    /// extrinsic directly if you need it.
    function putObjectMetadata(
        uint64 s3BucketId,
        string calldata key,
        bytes32 cid,
        uint64 size,
        string calldata contentType
    ) external;

    /// Delete an object.
    function deleteObjectMetadata(uint64 s3BucketId, string calldata key) external;

    /// Copy object metadata. Caller must own both buckets.
    function copyObjectMetadata(
        uint64 srcBucketId,
        string calldata srcKey,
        uint64 dstBucketId,
        string calldata dstKey
    ) external;
}
