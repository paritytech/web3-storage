// SPDX-License-Identifier: Apache-2.0

pragma solidity ^0.8.34;

import "./IDriveRegistry.sol";

/// @title SharedTeamDrive
/// @notice Example dApp showing how a contract owns a drive and manages its
///         membership through the drive-registry precompile. The contract's
///         substrate-mapped account is the drive owner on-chain; per-user
///         roles are tracked here so an off-chain client can query them
///         without crawling the bucket's membership list.
///
/// Lifecycle:
///   1. Whoever deploys + calls `createTeam` becomes the team admin. The
///      terms are negotiated off-chain with a provider (`POST /negotiate`)
///      using the *contract's* substrate-mapped account as `terms.owner`.
///   2. Admin can `invite(member, role)` and `kick(member)` — these update
///      the contract-side `memberRole` map and forward to
///      `DRIVE_REGISTRY.shareDrive` / `unshareDrive`.
///   3. Any member can `topUpForRenewal{value: ...}` to accumulate native
///      currency in the contract; payouts/extensions are out of scope for
///      v1 (would call into the storage-provider precompile to extend the
///      drive's underlying agreement).
///   4. `disband` deletes the drive and clears state. Admin only.
///
/// Role tags: 0 = Admin, 1 = Writer, 2 = Reader.
contract SharedTeamDrive {
    IDriveRegistry constant DRIVE_REGISTRY =
        IDriveRegistry(0x0000000000000000000000000000000009020000);

    uint64 public driveId;
    // `admin != address(0)` is the "team exists" sentinel — we can't use
    // `driveId != 0` because the chain assigns `drive_id` starting at 0
    // (the first drive ever created on a fresh chain has id 0).
    address public admin;
    mapping(address => uint8) public memberRole;
    uint256 public renewalPool;

    event TeamCreated(address indexed admin, uint64 driveId);
    event Invited(address indexed by, bytes32 member, uint8 role);
    event Kicked(address indexed by, bytes32 member);
    event RenewalDeposited(address indexed from, uint256 amount);
    event Disbanded(address indexed by, uint64 driveId);

    modifier onlyAdmin() {
        require(msg.sender == admin, "not admin");
        _;
    }

    /// Create the team's drive by redeeming provider-signed terms
    /// (`terms.owner` must be the contract's substrate-mapped account).
    /// `msg.value` funds the agreement payment reserve held by that account.
    /// Primary terms must not be bound to an existing bucket — the drive's
    /// bucket is created at redemption.
    function createTeam(
        string calldata name,
        bytes32 provider,
        IDriveRegistry.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64) {
        require(admin == address(0), "team already created");
        require(msg.value > 0, "must fund agreement");
        require(!terms.hasBucketId, "primary terms must not be bucket-bound");
        admin = msg.sender;
        driveId = DRIVE_REGISTRY.createDrive(name, provider, terms, signature);
        emit TeamCreated(msg.sender, driveId);
        return driveId;
    }

    /// Add a member to the drive at the given role.
    function invite(bytes32 member, uint8 role) external onlyAdmin {
        require(admin != address(0), "no team");
        DRIVE_REGISTRY.shareDrive(driveId, member, role);
        // memberRole keyed by EVM address; for off-chain UX, callers can
        // pass either an empty-mapped address (substrate-only member) or
        // the H160-mapped equivalent. v1 simply records the bytes32 hash
        // of the substrate id as a u160 mask for traceability.
        memberRole[address(uint160(uint256(member)))] = role;
        emit Invited(msg.sender, member, role);
    }

    /// Remove a member from the drive.
    function kick(bytes32 member) external onlyAdmin {
        require(admin != address(0), "no team");
        DRIVE_REGISTRY.unshareDrive(driveId, member);
        delete memberRole[address(uint160(uint256(member)))];
        emit Kicked(msg.sender, member);
    }

    /// Accept native-currency contributions for future agreement extension
    /// or top-up. v1 leaves the balance unused; a follow-up could wire the
    /// storage-provider precompile's topUpAgreement / extendAgreement.
    function topUpForRenewal() external payable {
        require(msg.value > 0, "must contribute");
        renewalPool += msg.value;
        emit RenewalDeposited(msg.sender, msg.value);
    }

    /// Tear down the team. Admin-only.
    function disband() external onlyAdmin {
        require(admin != address(0), "no team");
        uint64 id = driveId;
        DRIVE_REGISTRY.deleteDrive(id);
        driveId = 0;
        admin = address(0);
        emit Disbanded(msg.sender, id);
    }
}
