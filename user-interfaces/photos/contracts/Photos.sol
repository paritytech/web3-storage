// SPDX-License-Identifier: Apache-2.0

pragma solidity ^0.8.34;

import "./IWeb3Storage.sol";

/// @title Photos
/// @notice Per-user control plane for the Photos dApp. The contract creates one
///         Layer 0 bucket per user through the storage-provider precompile,
///         grants the user a Writer role so their browser can call the
///         provider's `/fs` API directly, and anchors the album-tree root CID
///         on-chain.
///
///         Origin model: precompile calls dispatch as
///         `RawOrigin::Signed(contract_account)`, so the *contract* is the
///         admin of every user's bucket. Per-user attribution lives here
///         (`libraries`, `bucketOwner`). The contract code enforces that only
///         the user manages their library.
///
/// Follows the `SharedTeamDrive.sol` pattern (bucket creation from
/// provider-signed terms plus a membership grant) and adds the on-chain root
/// anchor.
contract Photos {
    IWeb3Storage constant STORAGE =
        IWeb3Storage(0x0000000000000000000000000000000009010000);

    struct Library {
        uint64 bucketId;
        bytes32 rootCid;
        bool exists;
    }

    // user (EVM address: the caller's substrate-mapped account) → their library.
    mapping(address => Library) public libraries;
    // Ownership guard. `exists` (not `bucketId != 0`) is the sentinel because
    // the chain assigns bucket ids starting at 0.
    mapping(uint64 => address) public bucketOwner;

    event LibraryCreated(address indexed user, uint64 indexed bucketId, bytes32 provider);
    event RootUpdated(address indexed user, uint64 indexed bucketId, bytes32 rootCid);

    /// Create my library with a provider I chose. `msg.value` funds the
    /// agreement payment, reserved from the contract's balance when the
    /// precompile dispatches. The contract is the bucket admin and grants me
    /// (`userAccount`, my substrate AccountId32) a Writer role so my browser
    /// can upload and list directly against the provider's `/fs` API.
    ///
    /// `terms.owner` must be the contract's substrate-mapped account (the
    /// bucket admin). `terms.hasBucketId` must be false: the bucket is created
    /// at redemption.
    function createLibrary(
        bytes32 userAccount,
        bytes32 provider,
        IWeb3Storage.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64 bucketId) {
        require(!libraries[msg.sender].exists, "library exists");
        require(!terms.hasBucketId, "primary terms must not be bucket-bound");
        // A photo library is member-only by default.
        bucketId = STORAGE.createBucketWithPrimary(
            provider, terms, signature, IWeb3Storage.Visibility.Private
        );
        STORAGE.setMember(bucketId, userAccount, IWeb3Storage.Role.Writer);
        libraries[msg.sender] = Library(bucketId, bytes32(0), true);
        bucketOwner[bucketId] = msg.sender;
        emit LibraryCreated(msg.sender, bucketId, provider);
    }

    /// Anchor the current album-tree root on-chain after the client changed the
    /// tree off-chain (upload, new album, edit, delete). `rootCid` is the
    /// metadata Merkle root the client computes over the bucket's sorted
    /// (path, data_root, size) entries.
    function setRoot(bytes32 rootCid) external {
        Library storage lib = libraries[msg.sender];
        require(lib.exists, "no library");
        lib.rootCid = rootCid;
        emit RootUpdated(msg.sender, lib.bucketId, rootCid);
    }

    /// The UI reads this unsigned via `ReviveApi.call` (no signature, no gas)
    /// to detect state and to fetch the integrity anchor.
    function libraryOf(address user)
        external
        view
        returns (uint64 bucketId, bytes32 rootCid, bool exists)
    {
        Library memory l = libraries[user];
        return (l.bucketId, l.rootCid, l.exists);
    }
}
