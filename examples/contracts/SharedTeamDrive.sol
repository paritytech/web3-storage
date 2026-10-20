// SPDX-License-Identifier: Apache-2.0

pragma solidity ^0.8.34;

import "./IWeb3Storage.sol";

/// @title SharedTeamDrive
/// @notice Example dApp showing how a contract owns a drive and manages its
///         membership through the storage-provider precompile. A drive is a
///         plain Layer 0 bucket. The contract's substrate-mapped account is
///         the bucket admin on-chain; per-user roles are tracked here so an
///         off-chain client can query them without reading the bucket's
///         member list.
///
/// Lifecycle:
///   1. Whoever calls `createTeam` becomes the team admin. The terms are
///      negotiated off-chain with a provider (`POST /negotiate`) using the
///      *contract's* substrate-mapped account as `terms.owner`.
///   2. Admin can `invite(member, role)` and `kick(member)`. These update
///      the contract-side `memberRole` map and forward to
///      `WEB3_STORAGE.setMember` / `removeMember`.
///   3. Any member can `topUpForRenewal{value: ...}` to accumulate native
///      currency in the contract; payouts/extensions are out of scope for
///      v1 (would call the precompile's `extendAgreement`).
///
/// The bucket has no delete: it remains until its agreements expire.
///
/// Role tags: 0 = Admin, 1 = Writer, 2 = Reader.
contract SharedTeamDrive {
    IWeb3Storage constant WEB3_STORAGE =
        IWeb3Storage(0x0000000000000000000000000000000009010000);

    uint64 public bucketId;
    // `admin != address(0)` is the "team exists" sentinel. `bucketId != 0`
    // does not work: the chain assigns bucket ids starting at 0.
    address public admin;
    mapping(address => uint8) public memberRole;
    uint256 public renewalPool;

    event TeamCreated(address indexed admin, uint64 bucketId);
    event Invited(address indexed by, bytes32 member, uint8 role);
    event Kicked(address indexed by, bytes32 member);
    event RenewalDeposited(address indexed from, uint256 amount);

    modifier onlyAdmin() {
        require(msg.sender == admin, "not admin");
        _;
    }

    /// Create the team's drive by redeeming provider-signed terms
    /// (`terms.owner` must be the contract's substrate-mapped account).
    /// `msg.value` funds the agreement payment reserve held by that account.
    /// Primary terms must not be bound to an existing bucket: the bucket is
    /// created at redemption.
    function createTeam(
        bytes32 provider,
        IWeb3Storage.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64) {
        require(admin == address(0), "team already created");
        require(msg.value > 0, "must fund agreement");
        require(!terms.hasBucketId, "primary terms must not be bucket-bound");
        admin = msg.sender;
        // Team drives are member-only by default.
        bucketId = WEB3_STORAGE.createBucketWithPrimary(
            provider, terms, signature, IWeb3Storage.Visibility.Private
        );
        emit TeamCreated(msg.sender, bucketId);
        return bucketId;
    }

    /// Add a member to the drive at the given role, or change their role.
    function invite(bytes32 member, uint8 role) external onlyAdmin {
        WEB3_STORAGE.setMember(bucketId, member, IWeb3Storage.Role(role));
        // memberRole is keyed by the low 160 bits of the substrate account
        // id, for traceability only.
        memberRole[address(uint160(uint256(member)))] = role;
        emit Invited(msg.sender, member, role);
    }

    /// Remove a member from the drive.
    function kick(bytes32 member) external onlyAdmin {
        WEB3_STORAGE.removeMember(bucketId, member);
        delete memberRole[address(uint160(uint256(member)))];
        emit Kicked(msg.sender, member);
    }

    /// Accept native-currency contributions for future agreement extension
    /// or top-up. v1 leaves the balance unused; a follow-up could call the
    /// precompile's `topUpAgreement` / `extendAgreement`.
    function topUpForRenewal() external payable {
        require(msg.value > 0, "must contribute");
        renewalPool += msg.value;
        emit RenewalDeposited(msg.sender, msg.value);
    }
}
