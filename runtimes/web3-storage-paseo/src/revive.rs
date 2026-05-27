//! `pallet_revive` configuration: PolkaVM-based smart contracts with EVM
//! compatibility, exposing the storage-provider and drive-registry pallets to
//! Solidity contracts via custom precompiles.

use frame_support::{
    parameter_types,
    traits::{ConstBool, ConstU32, ConstU64},
};
use frame_system::EnsureSigned;
use pallet_revive::evm::runtime::EthExtra;
use sp_runtime::{generic, FixedU128, Perbill};

use crate::{
    paseo_constants::currency::UNIT, Address, Balance, Balances, Runtime, RuntimeCall,
    RuntimeEvent, RuntimeHoldReason, RuntimeOrigin, Signature, Timestamp, TxExtension,
};

/// 1/100 of a `UNIT`. Used as the per-cents fee target in `BlockRatioFee`.
pub const CENTS: Balance = UNIT / 100;

parameter_types! {
    pub const DepositPerItem: Balance = UNIT / 10;
    pub const DepositPerChildTrieItem: Balance = UNIT / 1_000;
    pub const DepositPerByte: Balance = UNIT / 1_000_000;
    pub CodeHashLockupDepositPercent: Perbill = Perbill::from_percent(30);
    pub const MaxEthExtrinsicWeight: FixedU128 = FixedU128::from_rational(9, 10);
}

impl pallet_revive::Config for Runtime {
    type Time = Timestamp;
    type Balance = Balance;
    type Currency = Balances;
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type RuntimeOrigin = RuntimeOrigin;
    type DepositPerItem = DepositPerItem;
    type DepositPerChildTrieItem = DepositPerChildTrieItem;
    type DepositPerByte = DepositPerByte;
    type WeightInfo = pallet_revive::weights::SubstrateWeight<Self>;
    // Empty in this commit; the storage-provider + drive-registry precompiles
    // land in the next commit and wire themselves into this tuple.
    type Precompiles = ();
    type AddressMapper = pallet_revive::AccountId32Mapper<Self>;
    type RuntimeMemory = ConstU32<{ 128 * 1024 * 1024 }>;
    type PVFMemory = ConstU32<{ 512 * 1024 * 1024 }>;
    type AllowEVMBytecode = ConstBool<true>;
    type UploadOrigin = EnsureSigned<Self::AccountId>;
    type InstantiateOrigin = EnsureSigned<Self::AccountId>;
    type RuntimeHoldReason = RuntimeHoldReason;
    type CodeHashLockupDepositPercent = CodeHashLockupDepositPercent;
    type ChainId = ConstU64<420_420_501>;
    type NativeToEthRatio = ConstU32<1_000_000>;
    type FindAuthor = <Runtime as pallet_authorship::Config>::FindAuthor;
    type FeeInfo = pallet_revive::evm::fees::Info<Address, Signature, EthExtraImpl>;
    type MaxEthExtrinsicWeight = MaxEthExtrinsicWeight;
    type DebugEnabled = ConstBool<false>;
    type GasScale = ConstU32<1000>;
    type OnBurn = ();
}

/// Builds the transaction extension stack for Ethereum-signed transactions
/// reaching the runtime via `pallet_revive::eth_transact`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EthExtraImpl;

impl EthExtra for EthExtraImpl {
    type Config = Runtime;
    type Extension = TxExtension;

    fn get_eth_extension(nonce: u32, tip: Balance) -> Self::Extension {
        (
            frame_system::CheckNonZeroSender::<Runtime>::new(),
            frame_system::CheckSpecVersion::<Runtime>::new(),
            frame_system::CheckTxVersion::<Runtime>::new(),
            frame_system::CheckGenesis::<Runtime>::new(),
            frame_system::CheckEra::from(generic::Era::Immortal),
            frame_system::CheckNonce::<Runtime>::from(nonce),
            frame_system::CheckWeight::<Runtime>::new(),
            pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
            pallet_revive::evm::tx_extension::SetOrigin::<Runtime>::new_from_eth_transaction(),
        )
            .into()
    }
}
