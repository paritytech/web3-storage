// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.34;

import "./IDriveRegistry.sol";

/// @title Photos
/// @notice Per-user control plane for the Photos dApp. The contract owns a
///         drive per user via the drive-registry precompile, grants the user a
///         Writer role so their browser can drive the provider's `/fs` API
///         directly, and anchors the album-tree root CID on-chain — a job the
///         bare drive registry does not do.
///
///         Origin model: precompile calls dispatch as
///         `RawOrigin::Signed(contract_account)`, so the *contract* owns every
///         user's drive. Per-user attribution lives here (`libraries`,
///         `driveOwner`). Custodial-by-ownership only: the transparent contract
///         enforces "only you manage your library".
///
/// Mirrors the proven `SharedTeamDrive.sol` pattern (drive creation from
/// provider-signed terms + a membership grant), adding the on-chain root anchor.
contract Photos {
    IDriveRegistry constant DRIVES =
        IDriveRegistry(0x0000000000000000000000000000000009020000);

    uint8 constant ROLE_WRITER = 1; // 0 = Admin, 1 = Writer, 2 = Reader

    struct Library {
        uint64 driveId;
        bytes32 rootCid;
        bool exists;
    }

    // user (EVM address — the caller's substrate-mapped account) → their library.
    mapping(address => Library) public libraries;
    // ownership guard. `exists` (not `driveId != 0`) is the sentinel because the
    // chain assigns drive ids starting at 0.
    mapping(uint64 => address) public driveOwner;

    event LibraryCreated(address indexed user, uint64 indexed driveId, bytes32 provider);
    event RootUpdated(address indexed user, uint64 indexed driveId, bytes32 rootCid);

    /// Create my library with a provider I chose. `msg.value` funds the
    /// agreement payment, reserved from the contract's balance when the
    /// precompile dispatches. The contract owns the drive and grants me
    /// (`userAccount`, my substrate AccountId32) a Writer role so my browser can
    /// upload/list directly against the provider's `/fs` API.
    ///
    /// `terms.owner` must be the contract's substrate-mapped account (the drive
    /// owner). For a primary agreement, `hasBucketId == false` — the drive's
    /// bucket is created at redemption.
    function createLibrary(
        bytes32 userAccount,
        string calldata name,
        bytes32 provider,
        IDriveRegistry.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64 driveId) {
        require(!libraries[msg.sender].exists, "library exists");
        require(!terms.hasBucketId, "primary terms must not be bucket-bound");
        driveId = DRIVES.createDrive(name, provider, terms, signature);
        DRIVES.shareDrive(driveId, userAccount, ROLE_WRITER);
        libraries[msg.sender] = Library(driveId, bytes32(0), true);
        driveOwner[driveId] = msg.sender;
        emit LibraryCreated(msg.sender, driveId, provider);
    }

    /// Anchor the current album-tree root on-chain after the client mutated the
    /// tree off-chain (upload / new album / edit / delete). `rootCid` is the
    /// metadata Merkle root the client computes itself over the drive's sorted
    /// (path, data_root, size) entries.
    function setRoot(bytes32 rootCid) external {
        Library storage lib = libraries[msg.sender];
        require(lib.exists, "no library");
        lib.rootCid = rootCid;
        emit RootUpdated(msg.sender, lib.driveId, rootCid);
    }

    /// UI reads this unsigned via `ReviveApi.call` (no signature, no gas) for
    /// state detection and to fetch the integrity anchor.
    function libraryOf(address user)
        external
        view
        returns (uint64 driveId, bytes32 rootCid, bool exists)
    {
        Library memory l = libraries[user];
        return (l.driveId, l.rootCid, l.exists);
    }
}
