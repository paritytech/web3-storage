// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.0;

/// @title IWeb3Storage
/// @notice ABI of the web3-storage precompile at
///         `0x0000000000000000000000000000000009010000` (matcher
///         `Fixed(0x0901)`). Mirrors
///         `precompiles/storage-provider-precompile/src/IWeb3Storage.sol`;
///         kept in sync manually. Substrate `AccountId32` is `bytes32`. The
///         EVM caller becomes the substrate-mapped owner via
///         `AccountId32Mapper`.
///
/// Role tags: 0 = Admin, 1 = Writer, 2 = Reader.
interface IWeb3Storage {
    function createBucket(uint32 minProviders) external returns (uint64 bucketId);
    function createBucketWithStorage(
        uint64 maxBytes,
        uint32 duration,
        uint128 maxPricePerByte
    ) external returns (uint64 bucketId);
    function freezeBucket(uint64 bucketId) external;
    function setMember(uint64 bucketId, bytes32 member, uint8 role) external;
    function removeMember(uint64 bucketId, bytes32 member) external;
    function requestPrimaryAgreement(
        uint64 bucketId,
        bytes32 provider,
        uint64 maxBytes,
        uint32 duration,
        uint128 maxPayment
    ) external;
    function topUpAgreement(
        uint64 bucketId,
        bytes32 provider,
        uint64 additionalBytes,
        uint128 maxPayment
    ) external;
    function extendAgreement(
        uint64 bucketId,
        bytes32 provider,
        uint32 additionalDuration,
        uint128 maxPayment
    ) external;
    function endAgreementPay(uint64 bucketId, bytes32 provider) external;
    function endAgreementBurn(uint64 bucketId, bytes32 provider, uint8 burnPercent) external;
    function challengeCheckpoint(
        uint64 bucketId,
        bytes32 provider,
        uint64 leafIndex,
        uint64 chunkIndex
    ) external;
}
