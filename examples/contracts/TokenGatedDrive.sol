// SPDX-License-Identifier: Apache-2.0

pragma solidity ^0.8.34;

import "./IWeb3Storage.sol";

/// @title TokenGatedDrive
/// @notice Example dApp: a Solidity contract owns a private Layer 0 bucket
///         and mints a transferable token per object key. Holding at least
///         one token makes the holder a `Reader` member of the bucket, so the
///         provider serves them its objects; transferring the last token
///         moves that membership to the receiver.
///
/// Holders are substrate accounts (`bytes32`). A holder calls `transfer` /
/// `burn` from the H160 that pallet-revive maps their account to (see
/// `_h160`).
///
/// Each distinct holder uses one bucket member slot. The runtime caps
/// members per bucket (`MaxMembers`); once the bucket is full, `mint` and
/// `transfer` to a new holder revert.
///
/// This is not ERC721: no approvals, no safeTransfer hooks, no metadata
/// extension. Access is per bucket, not per object, because bucket
/// membership is the access control the provider enforces.
///
/// Lifecycle:
///   1. Deployer negotiates terms off-chain with a provider (`POST
///      /negotiate`, `terms.owner` = the contract's substrate-mapped
///      account), then calls `initialize(publisherAccount, provider, terms,
///      signature)` with `msg.value` funding the agreement reserve. The
///      caller becomes the publisher; `publisherAccount` becomes a bucket
///      `Writer` so it can upload objects to the provider.
///   2. Publisher calls `mint(to, key)` per object. The first token of `to`
///      adds `to` as a bucket `Reader`.
///   3. Holder calls `transfer(to, tokenId)`. A holder left with no token
///      loses bucket membership.
///   4. Holder (or publisher, for a takedown) calls `burn(tokenId)`.
///
/// The bucket has no delete: it remains until its agreement expires.
contract TokenGatedDrive {
    IWeb3Storage constant WEB3_STORAGE =
        IWeb3Storage(0x0000000000000000000000000000000009010000);

    uint64 public bucketId;
    // `publisher != address(0)` is the "initialized" sentinel. `bucketId != 0`
    // does not work: the chain assigns bucket ids starting at 0.
    address public publisher;
    bytes32 public publisherAccount;

    // tokenId -> object key (the path inside the bucket).
    mapping(uint256 => string) public tokenKey;
    // tokenId -> current holder.
    mapping(uint256 => bytes32) private _holders;
    // holder -> token count.
    mapping(bytes32 => uint256) private _balances;
    uint256 public nextTokenId;

    event Initialized(address indexed publisher, uint64 bucketId);
    event Minted(bytes32 indexed to, uint256 indexed tokenId, string key);
    event Transfer(bytes32 indexed from, bytes32 indexed to, uint256 indexed tokenId);
    event Burned(uint256 indexed tokenId, string key);

    modifier onlyPublisher() {
        require(msg.sender == publisher, "not publisher");
        _;
    }

    /// Create the bucket by redeeming provider-signed terms
    /// (`terms.owner` must be the contract's substrate-mapped account) and
    /// grant `publisherAccount` `Writer`. One-shot. Primary terms must not be
    /// bound to an existing bucket: the bucket is created at redemption.
    function initialize(
        bytes32 publisherAccount_,
        bytes32 provider,
        IWeb3Storage.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64) {
        require(publisher == address(0), "already init");
        require(msg.value > 0, "must fund agreement");
        require(!terms.hasBucketId, "primary terms must not be bucket-bound");
        require(_h160(publisherAccount_) == msg.sender, "publisher account mismatch");
        publisher = msg.sender;
        publisherAccount = publisherAccount_;
        // Reads are token-gated, so the bucket must be member-only.
        bucketId = WEB3_STORAGE.createBucketWithPrimary(
            provider, terms, signature, IWeb3Storage.Visibility.Private
        );
        WEB3_STORAGE.setMember(bucketId, publisherAccount_, IWeb3Storage.Role.Writer);
        emit Initialized(msg.sender, bucketId);
        return bucketId;
    }

    /// Mint an access token for object `key` to substrate account `to`.
    function mint(bytes32 to, string calldata key)
        external
        onlyPublisher
        returns (uint256 tokenId)
    {
        require(to != bytes32(0), "zero to");
        tokenId = ++nextTokenId;
        tokenKey[tokenId] = key;
        _holders[tokenId] = to;
        _credit(to);
        emit Minted(to, tokenId, key);
        emit Transfer(bytes32(0), to, tokenId);
    }

    /// Transfer a token to substrate account `to`. Caller must be the
    /// holder's mapped H160.
    function transfer(bytes32 to, uint256 tokenId) external {
        bytes32 from = _holders[tokenId];
        require(from != bytes32(0), "no such token");
        require(msg.sender == _h160(from), "not holder");
        require(to != bytes32(0), "zero to");
        _holders[tokenId] = to;
        _debit(from);
        _credit(to);
        emit Transfer(from, to, tokenId);
    }

    /// Burn a token. Callable by its holder or by the publisher (takedown).
    /// Deleting the object's data is an off-chain provider call by the
    /// publisher.
    function burn(uint256 tokenId) external {
        bytes32 holder = _holders[tokenId];
        require(holder != bytes32(0), "no such token");
        require(msg.sender == _h160(holder) || msg.sender == publisher, "not allowed");
        string memory key = tokenKey[tokenId];
        delete _holders[tokenId];
        delete tokenKey[tokenId];
        _debit(holder);
        emit Burned(tokenId, key);
        emit Transfer(holder, bytes32(0), tokenId);
    }

    // --- Read surface --------------------------------------------------------

    function holderOf(uint256 tokenId) external view returns (bytes32) {
        return _holders[tokenId];
    }

    function balanceOf(bytes32 holder) external view returns (uint256) {
        return _balances[holder];
    }

    // --- Internal ------------------------------------------------------------

    /// H160 that pallet-revive maps a substrate account to: the first 20
    /// bytes for an Ethereum-derived account (last 12 bytes all `0xEE`),
    /// else `keccak256(account)[12..]`.
    function _h160(bytes32 account) private pure returns (address) {
        if (uint96(uint256(account)) == 0xeeeeeeeeeeeeeeeeeeeeeeee) {
            return address(bytes20(account));
        }
        return address(uint160(uint256(keccak256(abi.encodePacked(account)))));
    }

    /// Count a token for `holder`; add them as `Reader` on their first one.
    /// The publisher keeps `Writer` and is never downgraded.
    function _credit(bytes32 holder) private {
        if (_balances[holder]++ == 0 && holder != publisherAccount) {
            WEB3_STORAGE.setMember(bucketId, holder, IWeb3Storage.Role.Reader);
        }
    }

    /// Uncount a token for `holder`; remove their membership with the last one.
    function _debit(bytes32 holder) private {
        if (--_balances[holder] == 0 && holder != publisherAccount) {
            WEB3_STORAGE.removeMember(bucketId, holder);
        }
    }
}
