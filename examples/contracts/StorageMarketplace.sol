// SPDX-License-Identifier: Apache-2.0

pragma solidity ^0.8.34;

import "./IWeb3Storage.sol";

/// @title StorageMarketplace
/// @notice Example dApp showing how a Solidity contract uses the web3-storage
///         precompile to buy storage on behalf of its users.
///
/// Flow:
///   1. Off-chain, the user negotiates terms with a provider (`POST
///      /negotiate` on the provider node) using the *contract's*
///      substrate-mapped account as `terms.owner` — the contract is the
///      precompile caller, so the pallet binds the agreement to it.
///   2. User calls `buyStorage{value: nativeAmount}(provider, terms, signature)`.
///      `msg.value` funds the contract's substrate-mapped account; the
///      precompile verifies the provider's signature, reserves the payment
///      from that balance, and atomically creates the bucket + primary
///      agreement, returning the new bucket id. The contract records the
///      (user, bucket) pair so users can wind their agreement down later
///      without touching the chain directly.
///   3. To wind down, the user calls `endMyAgreement(bucketId, provider)`.
///      Only the original buyer can end their bucket's agreement.
///
/// Off-chain ops (uploads, challenges, checkpoints) are unchanged and bypass
/// this contract entirely — it only manages the on-chain control plane.
contract StorageMarketplace {
    /// Fixed precompile address. `AddressMatcher::Fixed(0x0901)` places the u16
    /// at bytes 16-17 of the H160 with a 0x0000 suffix at bytes 18-19, so the
    /// canonical address is `0x00000000000000000000000000000000_09010000`.
    IWeb3Storage constant WEB3_STORAGE =
        IWeb3Storage(0x0000000000000000000000000000000009010000);

    /// `bucketId → original buyer (EVM address)`.
    mapping(uint64 => address) public bucketOwner;

    event BucketBoughtFor(address indexed user, uint64 indexed bucketId);
    event AgreementEnded(address indexed user, uint64 indexed bucketId);

    /// Buy storage on behalf of `msg.sender` by redeeming provider-signed
    /// terms. The contract becomes the substrate-side bucket admin
    /// (`terms.owner` must be its substrate-mapped account); per-user
    /// ownership is tracked here. Primary terms must not be bound to an
    /// existing bucket — the bucket is created at redemption.
    function buyStorage(
        bytes32 provider,
        IWeb3Storage.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64 bucketId) {
        require(msg.value > 0, "msg.value must cover the agreement payment");
        require(!terms.hasBucketId, "primary terms must not be bucket-bound");
        bucketId = WEB3_STORAGE.establishStorageAgreement(
            provider,
            terms,
            signature
        );
        bucketOwner[bucketId] = msg.sender;
        emit BucketBoughtFor(msg.sender, bucketId);
    }

    /// End an agreement on a bucket the caller bought. Pays the provider in
    /// full (no burn). Provider must be passed as substrate `AccountId32`
    /// (`bytes32`); see README for how to obtain it.
    function endMyAgreement(uint64 bucketId, bytes32 provider) external {
        require(bucketOwner[bucketId] == msg.sender, "Not your bucket");
        WEB3_STORAGE.endAgreementPay(bucketId, provider);
        emit AgreementEnded(msg.sender, bucketId);
    }
}
