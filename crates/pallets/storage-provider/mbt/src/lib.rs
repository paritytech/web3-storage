// SPDX-License-Identifier: Apache-2.0

//! Model-based testing of the challenge protocol: replays traces of
//! `specs/quint/challenges.qnt` (issue #265) against the real pallet via
//! [`quint-connect`](https://github.com/informalsystems/quint-connect).
//!
//! The spec's `challengesCode` instance models the pallet *as implemented*
//! (including its known findings), so replay must PASS against today's
//! pallet. Its job is to lock the spec-to-code correspondence: the day a
//! finding is fixed in the pallet, replay fails until the spec moves with it.
//!
//! What is real and what is mirrored:
//! - Extrinsics, balances, challenges, snapshots, stake, pending counters —
//!   executed against the mock runtime and compared against the spec state
//!   after every step ([`SpecState`]).
//! - Ghost state the chain cannot see — which leaves a provider physically
//!   holds, off-chain signatures, the admin's deletion mark — is tracked in
//!   the driver and used to pick the concrete extrinsic arguments the
//!   model's adjudication assumed (e.g. *which* leaf proof to submit).
//! - The bucket admin (`Adm`) is registered as a zero-stake provider purely
//!   because `verify_signature` resolves signer keys through the `Providers`
//!   map; a non-provider admin could never mount a `Deleted` defense at all
//!   (a by-inspection finding recorded in the spec header and README).

pub mod mock;

use anyhow::{anyhow, bail, Context, Result};
use codec::Encode;
use frame_support::traits::{Get, Hooks};
use itf::value::{Record, Value};
use mock::{Balances, RuntimeOrigin, StorageProvider, System, Test};
use pallet_storage_provider::{
    Buckets, ChallengeResponse, Challenges, LastSweptChallengeBlock, PendingChallenges, Providers,
};
use quint_connect::{switch, Driver, State, Step};
use serde::Deserialize;
use sp_core::{sr25519, Pair as _, H256};
use sp_runtime::{BuildStorage, DispatchResult, MultiSignature};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};
use storage_primitives::{
    blake2_256, hash_children, verify_mmr_proof, AgreementTerms, ChallengeId, ChunkLocation,
    Commitment, CommitmentPayload, MerkleProof, MmrLeaf, MmrProof, ReplicaTerms,
};

/// The single bucket of the model.
const BUCKET: u64 = 0;
/// `AGREEMENT_END` of the spec instances: both agreements are set up at
/// anchor block 1 with duration 7.
const AGREEMENT_DURATION: u64 = 7;

// ─────────────────────────────────────────────────────────────────────────────
// Spec domain
// ─────────────────────────────────────────────────────────────────────────────

/// The spec's account sum type. Order matters only for `BTreeMap` keys.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
#[serde(tag = "tag")]
pub enum Acct {
    P1,
    P2,
    Adm,
    Pub,
    Tre,
}

impl Acct {
    pub fn account(self) -> u64 {
        match self {
            Acct::P1 => 1,
            Acct::P2 => 2,
            Acct::Adm => 3,
            Acct::Pub => 4,
            Acct::Tre => 999,
        }
    }

    fn provider_from_account(id: u64) -> Result<Self> {
        match id {
            1 => Ok(Acct::P1),
            2 => Ok(Acct::P2),
            other => bail!("account {other} is not a modeled provider"),
        }
    }

    fn challenger_from_account(id: u64) -> Result<Self> {
        match id {
            3 => Ok(Acct::Adm),
            4 => Ok(Acct::Pub),
            other => bail!("account {other} is not a modeled challenger"),
        }
    }
}

/// Deterministic sr25519 keypair per account; its public key is registered
/// on-chain so `verify_signature` / `verify_terms_signature` resolve it.
fn keypair(acct: Acct) -> sr25519::Pair {
    sr25519::Pair::from_seed(&[acct.account() as u8; 32])
}

/// The spec's `Commitment` record (roots are small opaque ints).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
pub struct CommitmentQ {
    pub root: i64,
    pub start: i64,
    pub count: i64,
}

pub const NO_COMMIT: CommitmentQ = CommitmentQ {
    root: 0,
    start: 0,
    count: 0,
};

/// The spec's `Response` sum type.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(tag = "tag")]
pub enum Rsp {
    RProof,
    RDeleted,
    RSuperseded,
}

// ─────────────────────────────────────────────────────────────────────────────
// MMR fixture
// ─────────────────────────────────────────────────────────────────────────────

/// Deterministic chunk bytes for sequence `seq` (one chunk per leaf, so the
/// leaf's `data_root` is just the chunk hash and the chunk proof is empty).
pub fn chunk_data(seq: u64) -> Vec<u8> {
    vec![0x10 + seq as u8; 8]
}

fn empty_proof() -> MerkleProof {
    MerkleProof {
        siblings: vec![],
        path: vec![],
    }
}

/// A real MMR over the leaf range of one model commitment: model roots 1/2/3
/// map to genuine `H256` roots and per-position proofs that
/// `verify_mmr_proof` accepts.
pub struct FixtureCommitment {
    pub model_root: i64,
    pub start: u64,
    pub count: u64,
    pub mmr_root: H256,
    peaks: Vec<H256>,
    leaves: Vec<MmrLeaf>,
    proofs: Vec<MerkleProof>,
}

impl FixtureCommitment {
    fn build(model_root: i64, start: u64, count: u64) -> Self {
        let leaves: Vec<MmrLeaf> = (0..count)
            .map(|pos| MmrLeaf {
                data_root: blake2_256(&chunk_data(start + pos)),
                data_size: 8,
                total_size: 8 * (pos + 1),
            })
            .collect();
        let hashes: Vec<H256> = leaves.iter().map(|l| blake2_256(&l.encode())).collect();

        // Standard MMR shape: perfect binary trees over left-to-right groups
        // sized by the set bits of `count`, descending; root = peaks bagged
        // right-to-left (mirroring `verify_mmr_proof`).
        let mut peaks = Vec::new();
        let mut proofs = Vec::new();
        let mut offset = 0usize;
        let mut rem = count as usize;
        while rem > 0 {
            let size = 1usize << (usize::BITS - 1 - rem.leading_zeros());
            let (peak, group_proofs) = perfect_tree(&hashes[offset..offset + size]);
            peaks.push(peak);
            proofs.extend(group_proofs);
            offset += size;
            rem -= size;
        }
        let mmr_root = peaks
            .iter()
            .rev()
            .fold(None, |acc, &peak| {
                Some(match acc {
                    None => peak,
                    Some(right) => hash_children(peak, right),
                })
            })
            .expect("count > 0");

        let fx = Self {
            model_root,
            start,
            count,
            mmr_root,
            peaks,
            leaves,
            proofs,
        };
        for pos in 0..count {
            assert!(
                verify_mmr_proof(&fx.mmr_proof(pos), &fx.mmr_root),
                "fixture proof for position {pos} of model root {model_root} must verify"
            );
        }
        fx
    }

    pub fn contains_seq(&self, seq: u64) -> bool {
        seq >= self.start && seq < self.start + self.count
    }

    pub fn commitment(&self) -> Commitment {
        Commitment {
            mmr_root: self.mmr_root,
            start_seq: self.start,
            leaf_count: self.count,
        }
    }

    fn mmr_proof(&self, pos: u64) -> MmrProof {
        MmrProof {
            peaks: self.peaks.clone(),
            leaf: self.leaves[pos as usize].clone(),
            leaf_proof: self.proofs[pos as usize].clone(),
        }
    }
}

/// Root and per-leaf proofs of a perfect binary tree over `hashes`
/// (`hashes.len()` is a power of two).
fn perfect_tree(hashes: &[H256]) -> (H256, Vec<MerkleProof>) {
    if hashes.len() == 1 {
        return (hashes[0], vec![empty_proof()]);
    }
    let half = hashes.len() / 2;
    let (left, left_proofs) = perfect_tree(&hashes[..half]);
    let (right, right_proofs) = perfect_tree(&hashes[half..]);
    let mut proofs = Vec::with_capacity(hashes.len());
    for mut p in left_proofs {
        p.siblings.push(right);
        p.path.push(false); // current node is the left child
        proofs.push(p);
    }
    for mut p in right_proofs {
        p.siblings.push(left);
        p.path.push(true);
        proofs.push(p);
    }
    (hash_children(left, right), proofs)
}

/// The three commitments of the model: `cA` appended into `cB`,
/// front-deleted into `cD`.
pub struct Fixture {
    commitments: Vec<FixtureCommitment>,
}

impl Fixture {
    pub fn new() -> Self {
        let commitments = vec![
            FixtureCommitment::build(1, 0, 2), // cA
            FixtureCommitment::build(2, 0, 3), // cB
            FixtureCommitment::build(3, 2, 2), // cD
        ];
        let mut roots: Vec<H256> = commitments.iter().map(|c| c.mmr_root).collect();
        roots.push(H256::zero());
        roots.sort();
        roots.dedup();
        assert_eq!(
            roots.len(),
            4,
            "fixture roots must be distinct and non-zero"
        );
        Self { commitments }
    }

    pub fn by_model_root(&self, root: i64) -> Result<&FixtureCommitment> {
        self.commitments
            .iter()
            .find(|c| c.model_root == root)
            .ok_or_else(|| anyhow!("unknown model root {root}"))
    }

    pub fn by_mmr_root(&self, root: H256) -> Result<&FixtureCommitment> {
        self.commitments
            .iter()
            .find(|c| c.mmr_root == root)
            .ok_or_else(|| anyhow!("unknown MMR root {root}"))
    }

    /// Model root for an on-chain root (0 = the spec's `NO_COMMIT`).
    pub fn model_root(&self, root: H256) -> Result<i64> {
        if root == H256::zero() {
            return Ok(0);
        }
        Ok(self.by_mmr_root(root)?.model_root)
    }

    /// Resolve a spec commitment record, checking its range matches the
    /// fixture (the spec only ever uses cA/cB/cD verbatim).
    pub fn resolve(&self, c: &CommitmentQ) -> Result<&FixtureCommitment> {
        let fx = self.by_model_root(c.root)?;
        if fx.start != c.start as u64 || fx.count != c.count as u64 {
            bail!(
                "commitment {c:?} does not match fixture range {}..{}",
                fx.start,
                fx.count
            );
        }
        Ok(fx)
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Driver
// ─────────────────────────────────────────────────────────────────────────────

fn ok(result: DispatchResult) -> Result<()> {
    result.map_err(|e| anyhow!("extrinsic failed: {e:?}"))
}

pub struct ChallengeDriver {
    ext: RefCell<sp_io::TestExternalities>,
    fixture: Fixture,
    /// Anchor (relay) block, mirroring the spec's `now`.
    now: u64,
    /// Ghost: sequence numbers each provider physically holds off-chain.
    leaves: BTreeMap<Acct, BTreeSet<u64>>,
    /// Ghost: the replica's last confirmed sync (spec keeps it after
    /// agreement teardown, so it cannot be read back from chain storage).
    last_sync: CommitmentQ,
    /// Ghost: highest `new_start_seq` the admin ever signed a deletion for.
    admin_deleted_to: u64,
    /// Announced deregistration blocks; the spec retains the value after
    /// `complete_deregister` removes the provider record.
    dereg_at: BTreeMap<Acct, i64>,
}

impl Default for ChallengeDriver {
    fn default() -> Self {
        Self {
            ext: RefCell::new(sp_io::TestExternalities::default()),
            fixture: Fixture::new(),
            now: 0,
            leaves: BTreeMap::new(),
            last_sync: NO_COMMIT,
            admin_deleted_to: 0,
            dereg_at: BTreeMap::new(),
        }
    }
}

impl ChallengeDriver {
    fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        self.ext.borrow_mut().execute_with(f)
    }

    fn sign_payload(&self, signer: Acct, commitment: Commitment) -> MultiSignature {
        let payload = CommitmentPayload::new(BUCKET, commitment).encode();
        MultiSignature::Sr25519(keypair(signer).sign(&payload))
    }

    /// `init`: rebuild genesis and replay the (unmodeled) setup phase the
    /// spec's `init` state assumes — registrations, one bucket, a primary
    /// and a replica agreement, all money-neutral (prices and quotas are
    /// zero-cost so balances land exactly on the spec's init values).
    pub fn init_chain(&mut self) -> Result<()> {
        let mut storage = frame_system::GenesisConfig::<Test>::default()
            .build_storage()
            .map_err(|e| anyhow!("genesis: {e}"))?;
        pallet_balances::GenesisConfig::<Test> {
            balances: vec![(1, 120), (2, 120), (3, 100), (4, 100)],
            dev_accounts: None,
        }
        .assimilate_storage(&mut storage)
        .map_err(|e| anyhow!("balances genesis: {e}"))?;
        *self.ext.borrow_mut() = sp_io::TestExternalities::from(storage);

        self.now = 1;
        self.leaves = BTreeMap::from([
            (Acct::P1, BTreeSet::from([0, 1, 2, 3])),
            (Acct::P2, BTreeSet::new()),
        ]);
        self.last_sync = NO_COMMIT;
        self.admin_deleted_to = 0;
        self.dereg_at = BTreeMap::new();

        self.with(|| -> Result<()> {
            System::set_block_number(1);

            for (acct, stake) in [(Acct::P1, 100), (Acct::P2, 100), (Acct::Adm, 0)] {
                ok(StorageProvider::register_provider(
                    RuntimeOrigin::signed(acct.account()),
                    b"/mbt".to_vec().try_into().expect("short multiaddr"),
                    keypair(acct)
                        .public()
                        .0
                        .to_vec()
                        .try_into()
                        .expect("32-byte key"),
                    stake,
                ))?;
            }

            // P2 must advertise a replica sync price to accept the replica
            // agreement; zero keeps every sync payment-neutral.
            ok(StorageProvider::update_provider_settings(
                RuntimeOrigin::signed(Acct::P2.account()),
                pallet_storage_provider::ProviderSettings::<Test> {
                    replica_sync_price: Some(0),
                    ..Default::default()
                },
            ))?;

            let primary_terms: pallet_storage_provider::AgreementTermsOf<Test> = AgreementTerms {
                owner: Acct::Adm.account(),
                max_bytes: 1,
                duration: AGREEMENT_DURATION,
                price_per_byte: 0,
                valid_until: 2,
                nonce: 1,
                bucket_id: None,
                replica_params: None,
            };
            let sig = sign_terms(Acct::P1, &primary_terms);
            ok(StorageProvider::establish_storage_agreement(
                RuntimeOrigin::signed(Acct::Adm.account()),
                Acct::P1.account(),
                primary_terms,
                sig,
            ))?;

            let replica_terms: pallet_storage_provider::AgreementTermsOf<Test> = AgreementTerms {
                owner: Acct::Adm.account(),
                max_bytes: 1,
                duration: AGREEMENT_DURATION,
                price_per_byte: 0,
                valid_until: 2,
                nonce: 1,
                bucket_id: Some(BUCKET),
                replica_params: Some(ReplicaTerms {
                    sync_balance: 0,
                    min_sync_interval: 0,
                    sync_price: 0,
                }),
            };
            let sig = sign_terms(Acct::P2, &replica_terms);
            ok(StorageProvider::establish_replica_agreement(
                RuntimeOrigin::signed(Acct::Adm.account()),
                BUCKET,
                Acct::P2.account(),
                replica_terms,
                sig,
            ))?;

            // First sweep call anchors the cursor at `now - 1`.
            <StorageProvider as Hooks<u64>>::on_initialize(1);
            Ok(())
        })
    }

    /// `advanceBlockBy`: move the anchor clock and run the timeout sweep to
    /// completion. One `on_initialize` per parachain block; consecutive
    /// parachain blocks can share an anchor block, so loop until the sweep
    /// cursor catches up (budget exhaustion parks it mid-range).
    pub fn advance(&mut self, jump: u64) -> Result<()> {
        self.now += jump;
        let now = self.now;
        self.with(|| -> Result<()> {
            System::set_block_number(now);
            for _ in 0..64 {
                <StorageProvider as Hooks<u64>>::on_initialize(now);
                if LastSweptChallengeBlock::<Test>::get() == Some(now - 1) {
                    return Ok(());
                }
            }
            bail!("sweep cursor failed to catch up to {}", now - 1)
        })
    }

    /// `signOffchainC` is purely off-chain: the trace guarantees the primary
    /// only signs over data it holds, and the driver signs payloads at the
    /// moment they are submitted on-chain, so there is nothing to record here.
    pub fn sign_offchain(&mut self, _c: CommitmentQ) -> Result<()> {
        Ok(())
    }

    /// `doCheckpointC`: the client (admin) submits the primary's signature.
    pub fn do_checkpoint(&mut self, c: CommitmentQ) -> Result<()> {
        let commitment = self.fixture.resolve(&c)?.commitment();
        let sig = self.sign_payload(Acct::P1, commitment);
        self.with(|| {
            ok(StorageProvider::checkpoint(
                RuntimeOrigin::signed(Acct::Adm.account()),
                BUCKET,
                commitment,
                vec![(Acct::P1.account(), sig)]
                    .try_into()
                    .expect("one signature"),
            ))
        })
    }

    /// `replicaSyncC`: the replica confirms sync to the current snapshot
    /// root (position 0 of `find_matching_root`).
    pub fn replica_sync(&mut self, c: CommitmentQ) -> Result<()> {
        let fx = self.fixture.resolve(&c)?;
        let mut roots = [None; 7];
        roots[0] = Some(fx.mmr_root);
        // The extrinsic's signature parameter is unused by the pallet.
        let sig = MultiSignature::Sr25519(keypair(Acct::P2).sign(b"unused"));
        self.with(|| {
            ok(StorageProvider::confirm_replica_sync(
                RuntimeOrigin::signed(Acct::P2.account()),
                BUCKET,
                roots,
                sig,
            ))
        })?;
        self.last_sync = c;
        let synced: Vec<u64> = (fx.start..fx.start + fx.count).collect();
        self.leaves
            .get_mut(&Acct::P2)
            .expect("P2 tracked")
            .extend(synced);
        Ok(())
    }

    /// `loseLeaf`: ghost-only — a provider silently loses a stored chunk.
    pub fn lose_leaf(&mut self, provider: Acct, s: u64) -> Result<()> {
        let held = self
            .leaves
            .get_mut(&provider)
            .ok_or_else(|| anyhow!("{provider:?} untracked"))?;
        if !held.remove(&s) {
            bail!("{provider:?} does not hold leaf {s}");
        }
        Ok(())
    }

    /// `adminDeleteTo`: ghost-only — the admin signs a deletion commitment
    /// raising `start_seq`. The concrete signature is produced fresh when a
    /// `Deleted` response is submitted.
    pub fn admin_delete_to(&mut self, new_start: u64) -> Result<()> {
        if new_start <= self.admin_deleted_to {
            bail!(
                "adminDeleteTo must increase ({} -> {new_start})",
                self.admin_deleted_to
            );
        }
        self.admin_deleted_to = new_start;
        Ok(())
    }

    pub fn challenge_checkpoint(&mut self, challenger: Acct, leaf_index: u64) -> Result<()> {
        self.with(|| {
            ok(StorageProvider::challenge_checkpoint(
                RuntimeOrigin::signed(challenger.account()),
                BUCKET,
                Acct::P1.account(),
                ChunkLocation {
                    leaf_index,
                    chunk_index: 0,
                },
            ))
        })
    }

    pub fn challenge_offchain(
        &mut self,
        challenger: Acct,
        c: CommitmentQ,
        leaf_index: u64,
    ) -> Result<()> {
        let commitment = self.fixture.resolve(&c)?.commitment();
        let sig = self.sign_payload(Acct::P1, commitment);
        self.with(|| {
            ok(StorageProvider::challenge_offchain(
                RuntimeOrigin::signed(challenger.account()),
                BUCKET,
                Acct::P1.account(),
                commitment,
                ChunkLocation {
                    leaf_index,
                    chunk_index: 0,
                },
                sig,
            ))
        })
    }

    pub fn challenge_replica(&mut self, challenger: Acct, leaf_index: u64) -> Result<()> {
        self.with(|| {
            ok(StorageProvider::challenge_replica(
                RuntimeOrigin::signed(challenger.account()),
                BUCKET,
                Acct::P2.account(),
                ChunkLocation {
                    leaf_index,
                    chunk_index: 0,
                },
            ))
        })
    }

    /// `respondAs`: submit the concrete response the model's adjudication
    /// assumed for `(deadline, index)`.
    pub fn respond(&mut self, k: (u64, u64), rsp: Rsp) -> Result<()> {
        let (deadline, index) = (k.0, k.1 as u16);
        let challenge = self
            .with(|| Challenges::<Test>::get(deadline, index))
            .ok_or_else(|| anyhow!("challenge ({deadline}, {index}) not found on chain"))?;
        let provider = Acct::provider_from_account(challenge.provider)?;

        let response = match rsp {
            Rsp::RProof => {
                let fx = self.fixture.by_mmr_root(challenge.mmr_root)?;
                let challenged_seq = challenge.start_seq + challenge.target.leaf_index;
                let held = &self.leaves[&provider];
                // Mirror of the spec's `proofVerifies` (code semantics):
                // prefer the challenged leaf; otherwise ANY held leaf under
                // the challenged root verifies, because `verify_mmr_proof`
                // never binds the proof to `challenge.target.leaf_index`
                // (finding 1). Holding nothing under the root leaves only a
                // demonstrably wrong proof — wrong chunk bytes — which the
                // pallet slashes as InvalidProof.
                let seq = if fx.contains_seq(challenged_seq) && held.contains(&challenged_seq) {
                    Some(challenged_seq)
                } else {
                    held.iter().copied().find(|s| fx.contains_seq(*s))
                };
                match seq {
                    Some(s) => ChallengeResponse::Proof {
                        chunk_data: chunk_data(s).try_into().expect("chunk fits"),
                        mmr_proof: fx.mmr_proof(s - fx.start),
                        chunk_proof: empty_proof(),
                    },
                    None => ChallengeResponse::Proof {
                        chunk_data: vec![0u8; 8].try_into().expect("chunk fits"),
                        mmr_proof: fx.mmr_proof(0),
                        chunk_proof: empty_proof(),
                    },
                }
            }
            Rsp::RDeleted => {
                // Present the admin-signed deletion at the highest
                // `new_start_seq` ever signed (0 = never signed anything, so
                // the claim cannot cover the challenged seq and is slashed as
                // InvalidDeletionClaim, matching `deletedVerifies`).
                let new_root = H256::repeat_byte(0xDD);
                let deletion = Commitment {
                    mmr_root: new_root,
                    start_seq: self.admin_deleted_to,
                    leaf_count: 0,
                };
                ChallengeResponse::Deleted {
                    new_mmr_root: new_root,
                    new_start_seq: self.admin_deleted_to,
                    admin: Acct::Adm.account(),
                    admin_signature: self.sign_payload(Acct::Adm, deletion),
                }
            }
            Rsp::RSuperseded => ChallengeResponse::Superseded,
        };

        self.with(|| {
            ok(StorageProvider::respond_to_challenge(
                RuntimeOrigin::signed(challenge.provider),
                ChallengeId { deadline, index },
                response,
            ))
        })
    }

    /// `announceDeregP`: on-chain this is two calls — the expired agreement
    /// must be torn down first so `committed_bytes` returns to zero.
    pub fn announce_dereg(&mut self, provider: Acct) -> Result<()> {
        self.with(|| -> Result<()> {
            ok(StorageProvider::claim_expired_agreement(
                RuntimeOrigin::signed(provider.account()),
                BUCKET,
            ))?;
            ok(StorageProvider::deregister_provider(RuntimeOrigin::signed(
                provider.account(),
            )))
        })?;
        let period: u64 =
            <Test as pallet_storage_provider::Config>::DeregisterAnnouncementPeriod::get();
        self.dereg_at.insert(provider, (self.now + period) as i64);
        Ok(())
    }

    pub fn complete_dereg(&mut self, provider: Acct) -> Result<()> {
        self.with(|| {
            ok(StorageProvider::complete_deregister(RuntimeOrigin::signed(
                provider.account(),
            )))
        })
    }
}

fn sign_terms(
    provider: Acct,
    terms: &pallet_storage_provider::AgreementTermsOf<Test>,
) -> MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
    MultiSignature::Sr25519(keypair(provider).sign(&hash))
}

impl Driver for ChallengeDriver {
    type State = SpecState;

    // Nondet-pick bindings must match the spec's camelCase names.
    #[allow(non_snake_case)]
    fn step(&mut self, step: &Step) -> quint_connect::Result {
        switch!(step {
            init => self.init_chain()?,
            advanceBlockBy(jump: u64) => self.advance(jump)?,
            signOffchainC(c: CommitmentQ) => self.sign_offchain(c)?,
            doCheckpointC(c: CommitmentQ) => self.do_checkpoint(c)?,
            replicaSyncC(c: CommitmentQ) => self.replica_sync(c)?,
            loseLeaf(provider: Acct, s: u64) => self.lose_leaf(provider, s)?,
            adminDeleteTo(newStart: u64) => self.admin_delete_to(newStart)?,
            challengeCheckpointAs(challenger: Acct, leafIndex: u64) =>
                self.challenge_checkpoint(challenger, leafIndex)?,
            challengeOffchainAs(challenger: Acct, c: CommitmentQ, leafIndex: u64) =>
                self.challenge_offchain(challenger, c, leafIndex)?,
            challengeReplicaAs(challenger: Acct, leafIndex: u64) =>
                self.challenge_replica(challenger, leafIndex)?,
            respondAs(k: (u64, u64), rsp: Rsp) => self.respond(k, rsp)?,
            announceDeregP(provider: Acct) => self.announce_dereg(provider)?,
            completeDeregP(provider: Acct) => self.complete_dereg(provider)?,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State comparison
// ─────────────────────────────────────────────────────────────────────────────

/// The chain-visible slice of the spec state, compared after every step.
///
/// Ghost-only spec fields are skipped on deserialization (serde ignores
/// unknown fields): `prov.*.leaves`, `prov.*.signed`, `adminDeletedTo`,
/// `log`, and `nextIdx` — the pallet prunes `NextChallengeIndex` for swept
/// deadlines while the spec retains entries forever; the allocator's visible
/// effects (challenge keys, the per-deadline cap) are covered by `open`.
#[derive(Debug, PartialEq, Deserialize)]
pub struct SpecState {
    pub now: i64,
    pub prov: BTreeMap<Acct, ProviderSt>,
    pub free: BTreeMap<Acct, i64>,
    pub reserved: BTreeMap<Acct, i64>,
    pub canonical: Snapshot,
    pub open: BTreeMap<(i64, i64), ChallengeSt>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ProviderSt {
    pub registered: bool,
    pub stake: i64,
    #[serde(rename = "deregisterAt")]
    pub deregister_at: i64,
    pub pending: i64,
    #[serde(rename = "lastSync")]
    pub last_sync: CommitmentQ,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Snapshot {
    pub c: CommitmentQ,
    pub signers: BTreeSet<Acct>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ChallengeSt {
    pub provider: Acct,
    pub challenger: Acct,
    pub root: i64,
    pub start: i64,
    pub count: i64,
    #[serde(rename = "leafIndex")]
    pub leaf_index: i64,
    pub deposit: i64,
}

impl State<ChallengeDriver> for SpecState {
    fn from_driver(d: &ChallengeDriver) -> Result<Self> {
        d.ext.borrow_mut().execute_with(|| {
            let mut prov = BTreeMap::new();
            for acct in [Acct::P1, Acct::P2] {
                let info = Providers::<Test>::get(acct.account());
                // The spec keeps `deregisterAt` after `complete_deregister`
                // removes the record; fall back to the driver's mirror then.
                let deregister_at = match &info {
                    Some(i) => i.deregister_at.map(|b| b as i64).unwrap_or(-1),
                    None => d.dereg_at.get(&acct).copied().unwrap_or(-1),
                };
                prov.insert(
                    acct,
                    ProviderSt {
                        registered: info.is_some(),
                        stake: info.map(|i| i.stake as i64).unwrap_or(0),
                        deregister_at,
                        pending: PendingChallenges::<Test>::get(acct.account()) as i64,
                        last_sync: if acct == Acct::P2 {
                            d.last_sync
                        } else {
                            NO_COMMIT
                        },
                    },
                );
            }

            let mut free = BTreeMap::new();
            let mut reserved = BTreeMap::new();
            for acct in [Acct::P1, Acct::P2, Acct::Adm, Acct::Pub, Acct::Tre] {
                free.insert(acct, Balances::free_balance(acct.account()) as i64);
                reserved.insert(acct, Balances::reserved_balance(acct.account()) as i64);
            }

            // `signers` mirrors the spec's snapshot bitfield abstraction:
            // P1 is the only primary and every checkpoint carries its
            // signature, so signers is {P1} exactly while a snapshot exists.
            // (The chain clears P1's bit when the agreement is torn down;
            // the spec keeps it — unobservable, since challenges require a
            // live agreement.)
            let bucket = Buckets::<Test>::get(BUCKET);
            let snapshot = bucket.as_ref().and_then(|b| b.snapshot.as_ref());
            let canonical = Snapshot {
                c: match snapshot {
                    Some(s) => CommitmentQ {
                        root: d.fixture.model_root(s.commitment.mmr_root)?,
                        start: s.commitment.start_seq as i64,
                        count: s.commitment.leaf_count as i64,
                    },
                    None => NO_COMMIT,
                },
                signers: match snapshot {
                    Some(_) => BTreeSet::from([Acct::P1]),
                    None => BTreeSet::new(),
                },
            };

            let mut open = BTreeMap::new();
            for (deadline, index, ch) in Challenges::<Test>::iter() {
                let fx = d.fixture.by_mmr_root(ch.mmr_root)?;
                open.insert(
                    (deadline as i64, index as i64),
                    ChallengeSt {
                        provider: Acct::provider_from_account(ch.provider)?,
                        challenger: Acct::challenger_from_account(ch.challenger)?,
                        root: fx.model_root,
                        start: ch.start_seq as i64,
                        count: fx.count as i64,
                        leaf_index: ch.target.leaf_index as i64,
                        deposit: ch.deposit as i64,
                    },
                );
            }

            Ok(SpecState {
                now: d.now as i64,
                prov,
                free,
                reserved,
                canonical,
                open,
            })
        })
    }

    fn from_spec(value: Value) -> Result<Self> {
        // Instance state vars come namespaced (`challengesCode::challenges::now`);
        // strip to the plain names the struct fields use.
        let Value::Record(record) = value else {
            bail!("expected the spec state to be a record");
        };
        let mut stripped = Record::new();
        for (key, val) in record {
            let plain = key.rsplit("::").next().unwrap_or(&key).to_string();
            stripped.insert(plain, val);
        }
        Self::deserialize(Value::Record(stripped)).context("failed to deserialize spec state")
    }
}
