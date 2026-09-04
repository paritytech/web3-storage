// SPDX-License-Identifier: Apache-2.0

//! Mock runtime for testing the storage provider pallet.

use crate as pallet_storage_provider;
use frame_support::{
    derive_impl,
    traits::{ConstU16, ConstU32, ConstU64, Hooks},
};
use sp_core::{Get, H256};
use sp_runtime::{
    traits::{BlakeTwo256, IdentityLookup},
    BuildStorage,
};
use std::sync::atomic::{AtomicU64, Ordering};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        StorageProvider: pallet_storage_provider,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = ();
    type BlockLength = ();
    type DbWeight = ();
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    type Nonce = u64;
    type Hash = H256;
    type Hashing = BlakeTwo256;
    type AccountId = u64;
    type Lookup = IdentityLookup<Self::AccountId>;
    type Block = Block;
    type RuntimeEvent = RuntimeEvent;
    type BlockHashCount = ConstU64<250>;
    type Version = ();
    type PalletInfo = PalletInfo;
    type AccountData = pallet_balances::AccountData<u64>;
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = ();
    type OnSetCode = ();
    type MaxConsumers = ConstU32<16>;
}

impl pallet_balances::Config for Test {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    type Balance = u64;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU64<1>;
    type AccountStore = System;
    type WeightInfo = ();
    type FreezeIdentifier = ();
    type MaxFreezes = ();
    type RuntimeHoldReason = ();
    type RuntimeFreezeReason = ();
    type DoneSlashHandler = ();
}

// Treasury account for testing
pub struct TestTreasury;
impl frame_support::traits::Get<u64> for TestTreasury {
    fn get() -> u64 {
        999 // Treasury is account 999 in tests
    }
}

impl pallet_storage_provider::Config for Test {
    type Currency = Balances;
    type Treasury = TestTreasury;
    type MinStakePerByte = ConstU64<1>; // 1 unit per byte
    type MaxMultiaddrLength = ConstU32<128>;
    type MaxMembers = ConstU32<100>;
    type MaxPrimaryProviders = ConstU32<5>;
    type MinProviderStake = ConstU64<100>;
    type MaxChunkSize = ConstU32<262144>; // 256 KiB
    type ChallengeTimeout = ConstU64<100>;
    type ChallengeDeposit = ConstU64<100>; // same value as pre-fix hardcoded constant — keeps existing tests' math intact
    type SettlementTimeout = ConstU64<50>;
    type RequestTimeout = ConstU64<50>;
    type MaxBucketsPerMember = ConstU32<100>;
    // Must be > ChallengeTimeout (100) AND > RequestTimeout (50) per the
    // pallet's `integrity_test`. 100 + 50 grace; small enough that tests can
    // advance past the period quickly.
    type DeregisterAnnouncementPeriod = ConstU64<150>;
    // Small cap so the cap-enforcement test can hit it without creating
    // thousands of challenges.
    type MaxChallengesPerDeadline = ConstU16<5>;
    type BlockNumberProvider = System;
    type AnchorBlockTimeMillis = ConstU64<6000>;
    type WeightInfo = ();
}

/// Build test externalities with default balances.
pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (1, 10_000),
            (2, 10_000),
            (3, 10_000),
            (4, 10_000),
            (5, 10_000),
            (6, 10_000),
            (7, 10_000),
            (8, 10_000),
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext: sp_io::TestExternalities = t.into();
    // Required by the benchmark test suite, which signs terms and
    // checkpoints through `sr25519_generate`/`sr25519_sign` host functions.
    ext.register_extension(sp_keystore::KeystoreExt::new(
        sp_keystore::testing::MemoryKeystore::new(),
    ));
    ext
}

/// Build test externalities with custom balances.
#[allow(dead_code)]
pub fn new_test_ext_with_balances(balances: Vec<(u64, u64)>) -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances,
        dev_accounts: None,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    t.into()
}

/// Build test externalities with custom balances and genesis providers.
pub fn new_test_ext_with_genesis_providers(
    balances: Vec<(u64, u64)>,
    providers: Vec<crate::GenesisProvider<Test>>,
) -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances,
        dev_accounts: None,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    pallet_storage_provider::GenesisConfig::<Test> {
        buckets: vec![],
        providers,
    }
    .assimilate_storage(&mut t)
    .unwrap();

    t.into()
}

/// Run to a specific block number, calling both System and StorageProvider hooks.
pub fn run_to_block(n: u64) {
    while System::block_number() < n {
        let current = System::block_number();
        <System as Hooks<u64>>::on_finalize(current);
        System::set_block_number(current + 1);
        <System as Hooks<u64>>::on_initialize(current + 1);
        <StorageProvider as Hooks<u64>>::on_initialize(current + 1);
    }
}

/// Helper: create a test public key (32 bytes).
pub fn test_public_key() -> frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> {
    vec![1u8; 32].try_into().unwrap()
}

/// Helper: register a provider with default settings and given stake.
pub fn register_provider(who: u64, stake: u64) {
    use frame_support::assert_ok;
    let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
    assert_ok!(StorageProvider::register_provider(
        RuntimeOrigin::signed(who),
        multiaddr.try_into().unwrap(),
        test_public_key(),
        stake
    ));
}

/// Helper: register a provider with custom settings.
pub fn register_provider_with_settings(
    who: u64,
    stake: u64,
    settings: crate::ProviderSettings<Test>,
) {
    use frame_support::assert_ok;
    register_provider(who, stake);
    assert_ok!(StorageProvider::update_provider_settings(
        RuntimeOrigin::signed(who),
        settings
    ));
}

/// Monotonic nonce for signed terms. The replay window only requires
/// per-provider uniqueness, so a process-wide counter satisfies it across
/// all tests.
static TERMS_NONCE: AtomicU64 = AtomicU64::new(1);

#[allow(dead_code)]
pub fn next_terms_nonce() -> u64 {
    TERMS_NONCE.fetch_add(1, Ordering::Relaxed)
}

/// Helper: deterministic keypair of any scheme for `provider`, stamped into
/// the provider's on-chain `public_key` so signatures verify.
pub fn provider_signer_with<P: sp_core::Pair>(provider: u64) -> P {
    use sp_core::crypto::ByteArray;
    let pair = P::from_seed_slice(&[provider as u8; 32]).expect("32-byte seed is valid");
    crate::Providers::<Test>::mutate(provider, |maybe_p| {
        if let Some(p) = maybe_p {
            p.public_key = pair.public().to_raw_vec().try_into().unwrap();
        }
    });
    pair
}

/// Helper: deterministic sr25519 keypair for `provider`, stamped into the
/// provider's on-chain `public_key` so terms signatures verify.
#[allow(dead_code)]
pub fn provider_signer(provider: u64) -> sp_core::sr25519::Pair {
    provider_signer_with(provider)
}

/// Helper: sign SCALE-encoded terms the way a provider quotes off-chain,
/// with any scheme whose signature converts into a `MultiSignature`
/// (`Eth`/`KeccakPair` has no such conversion — wrap it explicitly).
pub fn sign_terms_with<P: sp_core::Pair>(
    pair: &P,
    terms: &crate::AgreementTermsOf<Test>,
) -> sp_runtime::MultiSignature
where
    P::Signature: Into<sp_runtime::MultiSignature>,
{
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
    pair.sign(&hash).into()
}

/// Helper: sign SCALE-encoded terms the way a provider quotes off-chain.
#[allow(dead_code)]
pub fn sign_terms(
    pair: &sp_core::sr25519::Pair,
    terms: &crate::AgreementTermsOf<Test>,
) -> sp_runtime::MultiSignature {
    sign_terms_with(pair, terms)
}

/// Helper: attest sync roots the way a replica does for
/// `confirm_replica_sync` — a signature over the SCALE-encoded array from
/// the pair stamped as the provider's registered key.
pub fn sign_sync_roots(
    provider: u64,
    roots: &[Option<sp_core::H256>; 7],
) -> sp_runtime::MultiSignature {
    use codec::Encode;
    use sp_core::Pair as _;
    let pair = provider_signer(provider);
    sp_runtime::MultiSignature::Sr25519(pair.sign(&roots.encode()))
}

/// Helper: primary terms
/// + with a fresh nonce
/// + valid for the current RequestTimeout window.
#[allow(dead_code)]
pub fn primary_terms(
    owner: u64,
    max_bytes: u64,
    duration: u64,
    price_per_byte: u64,
) -> crate::AgreementTermsOf<Test> {
    storage_primitives::AgreementTerms {
        owner,
        max_bytes,
        duration,
        price_per_byte,
        valid_until: frame_system::Pallet::<Test>::block_number()
            .saturating_add(<Test as pallet_storage_provider::Config>::RequestTimeout::get()),
        nonce: next_terms_nonce(),
        bucket_id: None,
        replica_params: None,
    }
}

/// Helper: replica terms bound to `bucket_id`,
/// + valid for the current RequestTimeout window.
#[allow(dead_code)]
pub fn replica_terms(
    owner: u64,
    bucket_id: u64,
    max_bytes: u64,
    duration: u64,
    price_per_byte: u64,
    params: storage_primitives::ReplicaTerms<u64, u64>,
) -> crate::AgreementTermsOf<Test> {
    storage_primitives::AgreementTerms {
        owner,
        max_bytes,
        duration,
        price_per_byte,
        valid_until: frame_system::Pallet::<Test>::block_number()
            .saturating_add(<Test as pallet_storage_provider::Config>::RequestTimeout::get()),
        nonce: next_terms_nonce(),
        bucket_id: Some(bucket_id),
        replica_params: Some(params),
    }
}

/// Helper: signed primary terms at the provider's current settings price.
#[allow(dead_code)]
pub fn signed_primary_terms(
    provider: u64,
    owner: u64,
    max_bytes: u64,
    duration: u64,
) -> (crate::AgreementTermsOf<Test>, sp_runtime::MultiSignature) {
    let pair = provider_signer(provider);
    let price = crate::Providers::<Test>::get(provider)
        .map(|p| p.settings.price_per_byte)
        .unwrap_or(0);
    let terms = primary_terms(owner, max_bytes, duration, price);
    let sig = sign_terms(&pair, &terms);
    (terms, sig)
}

/// Helper: signed replica terms at the provider's current settings price.
#[allow(dead_code)]
pub fn signed_replica_terms(
    provider: u64,
    owner: u64,
    bucket_id: u64,
    max_bytes: u64,
    duration: u64,
    params: storage_primitives::ReplicaTerms<u64, u64>,
) -> (crate::AgreementTermsOf<Test>, sp_runtime::MultiSignature) {
    let pair = provider_signer(provider);
    let price = crate::Providers::<Test>::get(provider)
        .map(|p| p.settings.price_per_byte)
        .unwrap_or(0);
    let terms = replica_terms(owner, bucket_id, max_bytes, duration, price, params);
    let sig = sign_terms(&pair, &terms);
    (terms, sig)
}

/// Helper: create a bare bucket (no agreement). Returns bucket_id.
///
/// Public visibility: the existing suite exercises stranger challenges and
/// was written under pre-visibility (open) semantics. Private-bucket tests
/// create their buckets explicitly.
#[allow(dead_code)]
pub fn create_bucket(admin: u64, min_providers: u32) -> u64 {
    StorageProvider::create_bucket_internal(
        &admin,
        min_providers,
        None,
        storage_primitives::Visibility::Public,
    )
    .expect("create_bucket_internal succeeds")
}

/// Helper: redeem signed primary terms, creating the bucket together with
/// its Primary agreement. Returns bucket_id.
pub fn setup_agreement(provider: u64, client: u64, max_bytes: u64, duration: u64) -> u64 {
    use frame_support::assert_ok;
    let (terms, sig) = signed_primary_terms(provider, client, max_bytes, duration);
    assert_ok!(StorageProvider::establish_storage_agreement(
        RuntimeOrigin::signed(client),
        provider,
        terms,
        sig,
        // Public: preserves the open (pre-visibility) semantics the existing
        // suite was written under; Private-bucket tests opt in explicitly.
        storage_primitives::Visibility::Public,
    ));
    crate::NextBucketId::<Test>::get() - 1
}

/// Helper: redeem signed replica terms against an existing bucket.
#[allow(dead_code)]
pub fn setup_replica_agreement(
    provider: u64,
    client: u64,
    bucket_id: u64,
    max_bytes: u64,
    duration: u64,
    params: storage_primitives::ReplicaTerms<u64, u64>,
) {
    use frame_support::assert_ok;
    let (terms, sig) =
        signed_replica_terms(provider, client, bucket_id, max_bytes, duration, params);
    assert_ok!(StorageProvider::establish_replica_agreement(
        RuntimeOrigin::signed(client),
        bucket_id,
        provider,
        terms,
        sig
    ));
}

/// Helper: register `provider` as an additional primary on an existing
/// bucket via direct storage. `establish_storage_agreement` always creates
/// a fresh single-primary bucket, so multi-primary shapes are synthesized.
#[allow(dead_code)]
pub fn add_primary_to_bucket(provider: u64, owner: u64, bucket_id: u64, max_bytes: u64) {
    let anchor_block = System::block_number();
    crate::Buckets::<Test>::mutate(bucket_id, |maybe_bucket| {
        if let Some(bucket) = maybe_bucket {
            let _ = bucket.primary_providers.try_push(provider);
        }
    });
    crate::StorageAgreements::<Test>::insert(
        bucket_id,
        provider,
        crate::StorageAgreement::<Test> {
            owner,
            max_bytes,
            payment_locked: 0,
            price_per_byte: 0,
            expires_at: anchor_block + 200,
            extensions_blocked: false,
            role: storage_primitives::ProviderRole::Primary,
            started_at: anchor_block,
        },
    );
    crate::Providers::<Test>::mutate(provider, |maybe_p| {
        if let Some(p) = maybe_p {
            p.committed_bytes += max_bytes;
            p.stats.agreements_total += 1;
        }
    });
}
