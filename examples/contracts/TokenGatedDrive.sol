// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.34;

import "./IS3Registry.sol";

/// @title TokenGatedDrive
/// @notice Example dApp: a Solidity contract owns an S3 bucket on-chain and
///         mints a transferable NFT-like token for every object stored in
///         it. Holding a token represents off-chain access to that object;
///         transferring the token transfers access.
///
/// This is a minimal ERC721-shaped surface — `balanceOf`, `ownerOf`, and a
/// transfer function — not a full ERC721 (no approvals, no safeTransfer
/// hooks, no metadata extension). The goal is to demonstrate the access-
/// gating pattern, not ship a marketplace-ready NFT.
///
/// Lifecycle:
///   1. Deployer negotiates terms off-chain with a provider (`POST
///      /negotiate`, `terms.owner` = the contract's substrate-mapped
///      account), then calls `initialize(name, provider, terms, signature)`
///      with `msg.value` funding the agreement reserve. Becomes the
///      publisher.
///   2. Publisher calls `mint(buyer, key, cid, size, contentType)` per
///      object — `putObjectMetadata` registers the CID, contract assigns
///      tokenId and transfers ownership to `buyer`.
///   3. Holder calls `transfer(to, tokenId)` to hand off access.
///   4. Holder (or publisher, if reclaiming) calls `burn(tokenId)` to
///      delete the object metadata and free the slot.
///   5. Publisher can `shutdown()` once every token is burned — empties
///      the bucket and deletes it.
contract TokenGatedDrive {
    IS3Registry constant S3_REGISTRY =
        IS3Registry(0x0000000000000000000000000000000009030000);

    uint64 public s3BucketId;
    // `publisher != address(0)` is the "initialized" sentinel — the chain
    // assigns `s3_bucket_id` starting at 0, so we can't rely on
    // `s3BucketId != 0`.
    address public publisher;

    // tokenId -> object key (the off-chain path inside the bucket).
    mapping(uint256 => string) public tokenKey;
    // tokenId -> current owner.
    mapping(uint256 => address) private _owners;
    // owner -> count.
    mapping(address => uint256) private _balances;
    uint256 public nextTokenId;
    uint256 public outstandingTokens;

    event Initialized(address indexed publisher, uint64 s3BucketId);
    event Minted(address indexed to, uint256 indexed tokenId, string key, bytes32 cid);
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event Burned(uint256 indexed tokenId, string key);
    event Shutdown(address indexed by, uint64 s3BucketId);

    modifier onlyPublisher() {
        require(msg.sender == publisher, "not publisher");
        _;
    }

    /// Bootstrap the bucket by redeeming provider-signed terms
    /// (`terms.owner` must be the contract's substrate-mapped account).
    /// One-shot.
    function initialize(
        string calldata name,
        bytes32 provider,
        IS3Registry.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64) {
        require(publisher == address(0), "already init");
        require(msg.value > 0, "must fund agreement");
        publisher = msg.sender;
        s3BucketId = S3_REGISTRY.createS3Bucket(name, provider, terms, signature);
        emit Initialized(msg.sender, s3BucketId);
        return s3BucketId;
    }

    /// Register an object and mint its access token to `to`.
    function mint(
        address to,
        string calldata key,
        bytes32 cid,
        uint64 size,
        string calldata contentType
    ) external onlyPublisher returns (uint256 tokenId) {
        require(publisher != address(0), "not initialized");
        require(to != address(0), "zero to");
        S3_REGISTRY.putObjectMetadata(s3BucketId, key, cid, size, contentType);
        tokenId = ++nextTokenId;
        tokenKey[tokenId] = key;
        _owners[tokenId] = to;
        _balances[to]++;
        outstandingTokens++;
        emit Minted(to, tokenId, key, cid);
        emit Transfer(address(0), to, tokenId);
    }

    function transfer(address to, uint256 tokenId) external {
        address owner = _owners[tokenId];
        require(owner == msg.sender, "not owner");
        require(to != address(0), "zero to");
        _balances[owner]--;
        _balances[to]++;
        _owners[tokenId] = to;
        emit Transfer(owner, to, tokenId);
    }

    /// Burn a token and delete the underlying S3 object metadata. Callable
    /// by the token's owner (the holder relinquishes their copy) or by the
    /// publisher (e.g. takedown).
    function burn(uint256 tokenId) external {
        address owner = _owners[tokenId];
        require(owner != address(0), "no such token");
        require(msg.sender == owner || msg.sender == publisher, "not allowed");
        string memory key = tokenKey[tokenId];
        S3_REGISTRY.deleteObjectMetadata(s3BucketId, key);
        delete _owners[tokenId];
        delete tokenKey[tokenId];
        _balances[owner]--;
        outstandingTokens--;
        emit Burned(tokenId, key);
        emit Transfer(owner, address(0), tokenId);
    }

    /// Tear down once every token has been burned.
    function shutdown() external onlyPublisher {
        require(publisher != address(0), "no bucket");
        require(outstandingTokens == 0, "tokens outstanding");
        uint64 id = s3BucketId;
        S3_REGISTRY.deleteS3Bucket(id);
        s3BucketId = 0;
        publisher = address(0);
        emit Shutdown(msg.sender, id);
    }

    // --- ERC721-shaped read surface (subset) -------------------------------

    function ownerOf(uint256 tokenId) external view returns (address) {
        return _owners[tokenId];
    }

    function balanceOf(address holder) external view returns (uint256) {
        return _balances[holder];
    }
}
