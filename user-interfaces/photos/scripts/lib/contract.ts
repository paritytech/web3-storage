// SPDX-License-Identifier: GPL-3.0-only
//
// Photos-specific contract glue: reshape a provider-signed quote into the
// drive-registry precompile's `PrimitiveAgreementTerms` ABI struct, and derive
// the substrate account of a deployed contract from its H160. This is NOT
// general SDK material (it is precompile-shaped), so it stays local to the app,
// mirroring `examples/papi/sc-support.ts`.
//
// The chain-generic revive helpers (deployContract / callContract / encodeCall /
// decodeContractEmitted / substrateToH160 / h160ToSubstrate / ensureAccountMapped)
// come from `@web3-storage/sdk/revive`.

import { asHex, negotiateTerms, type ChainSigner } from "@web3-storage/sdk";

/** Owner-shaped value the negotiate helper needs (a signer or a mapped account). */
export interface Owner {
  publicKey: Uint8Array;
  address: string;
}

/** Mirror of `IDriveRegistry.PrimitiveAgreementTerms` for viem ABI encoding. */
export interface PrimitiveAgreementTerms {
  owner: `0x${string}`;
  maxBytes: bigint;
  duration: number;
  pricePerByte: bigint;
  validUntil: number;
  nonce: bigint;
  hasBucketId: boolean;
  bucketId: bigint;
  hasReplicaParams: boolean;
  replicaParams: { syncBalance: bigint; minSyncInterval: number; syncPrice: bigint };
}

export interface SignedTerms {
  terms: PrimitiveAgreementTerms;
  signature: `0x${string}`;
}

/**
 * Negotiate primary terms with `owner` as the bound account, shaped for the
 * precompile's `PrimitiveAgreementTerms` ABI struct. `owner` must be the account
 * the pallet will see as origin — for Photos, the contract's substrate-mapped
 * account (`h160ToSubstrate(deployed.addressBytes)`).
 */
export async function negotiatePrecompileTerms(
  providerUrl: string,
  owner: Owner | ChainSigner,
  { maxBytes, duration, pricePerByte }: { maxBytes: bigint; duration: number; pricePerByte: bigint },
): Promise<SignedTerms> {
  const signed = await negotiateTerms(providerUrl, {
    owner: owner.address,
    max_bytes: BigInt(maxBytes),
    duration,
    price_per_byte: pricePerByte,
    replica_params: null,
    bucket_id: null,
  });
  const t = signed.terms;
  const rp = t.replica_params;
  const bucket = t.bucket_id;
  return {
    terms: {
      owner: asHex(owner.publicKey),
      maxBytes: BigInt(t.max_bytes),
      duration: Number(t.duration),
      pricePerByte: BigInt(t.price_per_byte),
      validUntil: Number(t.valid_until),
      nonce: BigInt(t.nonce),
      hasReplicaParams: rp != null,
      replicaParams: {
        syncBalance: BigInt(rp?.sync_balance ?? 0),
        minSyncInterval: Number(rp?.min_sync_interval ?? 0),
        syncPrice: BigInt(rp?.sync_price ?? 0),
      },
      hasBucketId: bucket != null,
      bucketId: BigInt(bucket ?? 0),
    },
    signature: asHex(signed.signature),
  };
}
