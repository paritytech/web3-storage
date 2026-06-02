// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.0;

/// @title IS3Registry
/// @notice Solidity interface for the web3-storage `pallet_s3_registry`
///         precompile. The EVM caller's substrate-mapped account becomes the
///         bucket owner; bucket names follow the S3 convention (3-63 chars,
///         lowercase alphanumeric + hyphens). `cid` is a substrate `H256`.
interface IS3Registry {
    /// Create an S3 bucket (no storage agreement yet).
    function createS3Bucket(string calldata name, uint32 minProviders)
        external returns (uint64 s3BucketId);

    /// Create an S3 bucket and atomically open a primary storage agreement.
    /// `msg.value` (substrate units, via NativeToEthRatio) funds the
    /// contract's account so the pallet can reserve the payment.
    function createS3BucketWithStorage(
        string calldata name,
        uint64 maxCapacity,
        uint32 duration,
        uint128 maxPayment
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
