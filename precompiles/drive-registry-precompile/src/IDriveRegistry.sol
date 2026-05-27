// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.0;

/// @title IDriveRegistry
/// @notice Solidity interface for the web3-storage `pallet_drive_registry`
///         precompile. Substrate `AccountId32` values cross the boundary as
///         `bytes32`; the EVM caller's substrate-mapped account becomes the
///         drive owner.
///
/// Role tags: 0 = Admin, 1 = Writer, 2 = Reader.
interface IDriveRegistry {
    /// Create a new drive (auto-allocates a Layer 0 bucket and selects providers).
    ///
    /// - `name` may be empty (treated as `None`).
    /// - `minProviders == 0` means "use runtime default"; any value > 0 is
    ///   forwarded as `Some(n)`.
    ///
    /// Returns the new drive id.
    function createDrive(
        string calldata name,
        uint64 maxCapacity,
        uint32 storagePeriod,
        uint128 payment,
        uint8 minProviders
    ) external returns (uint64 driveId);

    /// Delete a drive, refunding any remaining payment to the owner.
    function deleteDrive(uint64 driveId) external;

    /// Share a drive with another account.
    function shareDrive(uint64 driveId, bytes32 member, uint8 role) external;

    /// Remove a previously-shared member from a drive.
    function unshareDrive(uint64 driveId, bytes32 member) external;
}
