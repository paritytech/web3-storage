// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.0;

import "./IS3Registry.sol";

/// @title MutableNotebook
/// @notice hack.md-shaped example: per-user file lists with full edit history.
///
/// The contract owns one S3 bucket. Per-key CID updates flow through the S3
/// Registry precompile, which already gives us a mutable on-chain pointer and
/// a pallet event on every put. The contract layers per-user authorship, a
/// revision counter, optimistic-concurrency-guarded updates, and indexed
/// `keyHash` events so a UI / indexer can replay history cheaply.
///
/// The actual bytes live off-chain in the provider's content-addressed store.
/// Old CIDs stay fetchable via `GET /content?data_root=<old_cid>` as long as
/// the bucket's storage agreement is alive.
contract MutableNotebook {
    IS3Registry constant S3 =
        IS3Registry(0x0000000000000000000000000000000009030000);

    uint64 public s3BucketId;
    address public admin;
    uint64 public outstandingFiles;

    struct FileMeta {
        address author;
        bytes32 currentCid;
        uint32 revision;
        uint64 size;
        uint64 lastUpdatedBlock;
        bool exists;
    }

    mapping(bytes32 => FileMeta) public files;
    mapping(bytes32 => string) public fileKey;
    mapping(address => bytes32[]) private _userIndex;

    event NotebookInitialized(address indexed admin, uint64 s3BucketId);
    event FileCreated(
        address indexed author,
        bytes32 indexed keyHash,
        string key,
        bytes32 cid,
        uint64 size,
        string contentType
    );
    event FileUpdated(
        address indexed author,
        bytes32 indexed keyHash,
        string key,
        bytes32 oldCid,
        bytes32 newCid,
        uint32 newRevision,
        string contentType,
        string commitMessage
    );
    event FileDeleted(
        address indexed author,
        bytes32 indexed keyHash,
        string key,
        bytes32 lastCid,
        uint32 lastRevision
    );
    event Shutdown(address indexed by, uint64 s3BucketId);

    error NotInitialized();
    error AlreadyInitialized();
    error NotAdmin();
    error NotAuthor();
    error FileNotFound();
    error FileAlreadyExists();
    error StaleRevision(uint32 expected, uint32 actual);
    error EmptyKey();
    error NotEmpty(uint64 stillStanding);

    function initialize(
        string calldata name,
        uint64 maxCapacity,
        uint32 duration,
        uint128 maxPayment
    ) external payable returns (uint64) {
        if (admin != address(0)) revert AlreadyInitialized();
        admin = msg.sender;
        s3BucketId = S3.createS3BucketWithStorage(
            name,
            maxCapacity,
            duration,
            maxPayment
        );
        emit NotebookInitialized(msg.sender, s3BucketId);
        return s3BucketId;
    }

    function createFile(
        string calldata key,
        bytes32 cid,
        uint64 size,
        string calldata contentType
    ) external {
        if (admin == address(0)) revert NotInitialized();
        if (bytes(key).length == 0) revert EmptyKey();
        bytes32 keyHash = keccak256(bytes(key));
        if (files[keyHash].exists) revert FileAlreadyExists();

        S3.putObjectMetadata(s3BucketId, key, cid, size, contentType);

        files[keyHash] = FileMeta({
            author: msg.sender,
            currentCid: cid,
            revision: 1,
            size: size,
            lastUpdatedBlock: uint64(block.number),
            exists: true
        });
        fileKey[keyHash] = key;
        _userIndex[msg.sender].push(keyHash);
        outstandingFiles += 1;

        emit FileCreated(msg.sender, keyHash, key, cid, size, contentType);
    }

    /// Optimistic concurrency: `expectedRevision` must match the current
    /// revision or the call reverts. Lets concurrent edits surface as a
    /// clean `StaleRevision` revert instead of a silent overwrite.
    function updateFile(
        string calldata key,
        bytes32 newCid,
        uint64 size,
        string calldata contentType,
        uint32 expectedRevision,
        string calldata commitMessage
    ) external {
        if (admin == address(0)) revert NotInitialized();
        bytes32 keyHash = keccak256(bytes(key));
        FileMeta storage meta = files[keyHash];
        if (!meta.exists) revert FileNotFound();
        if (meta.author != msg.sender) revert NotAuthor();
        if (meta.revision != expectedRevision) {
            revert StaleRevision(expectedRevision, meta.revision);
        }

        bytes32 oldCid = meta.currentCid;
        S3.putObjectMetadata(s3BucketId, key, newCid, size, contentType);

        meta.currentCid = newCid;
        meta.revision = expectedRevision + 1;
        meta.size = size;
        meta.lastUpdatedBlock = uint64(block.number);

        emit FileUpdated(
            msg.sender,
            keyHash,
            key,
            oldCid,
            newCid,
            meta.revision,
            contentType,
            commitMessage
        );
    }

    function deleteFile(string calldata key) external {
        if (admin == address(0)) revert NotInitialized();
        bytes32 keyHash = keccak256(bytes(key));
        FileMeta storage meta = files[keyHash];
        if (!meta.exists) revert FileNotFound();
        if (meta.author != msg.sender && msg.sender != admin) revert NotAuthor();

        S3.deleteObjectMetadata(s3BucketId, key);

        bytes32 lastCid = meta.currentCid;
        uint32 lastRevision = meta.revision;
        address author = meta.author;

        meta.currentCid = bytes32(0);
        meta.exists = false;
        outstandingFiles -= 1;

        emit FileDeleted(author, keyHash, key, lastCid, lastRevision);
    }

    /// Tears down the bucket once every file has been deleted. The S3
    /// Registry pallet rejects deleting a non-empty bucket; checking here
    /// gives a clearer revert reason.
    function shutdown() external {
        if (msg.sender != admin) revert NotAdmin();
        if (outstandingFiles != 0) revert NotEmpty(outstandingFiles);
        uint64 id = s3BucketId;
        S3.deleteS3Bucket(id);
        s3BucketId = 0;
        admin = address(0);
        emit Shutdown(msg.sender, id);
    }

    function getFile(string calldata key)
        external
        view
        returns (FileMeta memory)
    {
        bytes32 keyHash = keccak256(bytes(key));
        FileMeta memory meta = files[keyHash];
        if (!meta.exists) revert FileNotFound();
        return meta;
    }

    function listFiles(address author)
        external
        view
        returns (bytes32[] memory)
    {
        return _userIndex[author];
    }
}
