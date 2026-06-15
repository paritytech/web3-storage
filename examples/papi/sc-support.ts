/**
 * Demo-only glue for the smart-contract / precompile examples. This is NOT
 * general SDK material: it reshapes a provider-signed quote into the
 * precompiles' `PrimitiveAgreementTerms` ABI struct, and derives the
 * substrate account of a deployed contract from its H160.
 *
 * The chain-generic revive helpers (deployContract / callContract /
 * encodeCall / decodeContractEmitted / substrateToH160 / ensureAccountMapped)
 * live in `@web3-storage/sdk/revive`; only this contract glue stays local to
 * the examples.
 */

import { ss58Address } from "@polkadot-labs/hdkd-helpers";
import { negotiateTerms, toHex, type ChainSigner } from "@web3-storage/sdk";

/**
 * Derive a contract's substrate account from its H160 via the
 * `AccountId32Mapper` fallback rule: the 20 address bytes followed by 12
 * `0xee` padding bytes. Use this to name a deployed contract's account as the
 * `owner` of terms it will redeem (e.g. `h160ToSubstrate(deployed.addressBytes)`).
 */
export function h160ToSubstrate(addressBytes: Uint8Array): {
  publicKey: Uint8Array;
  address: string;
} {
  const publicKey = new Uint8Array(32).fill(0xee);
  publicKey.set(addressBytes, 0);
  return { publicKey, address: ss58Address(publicKey) };
}

/**
 * POST /negotiate on the provider node and shape the signed bundle for the
 * precompiles' `PrimitiveAgreementTerms` ABI struct. `owner` must be the
 * account the pallet will see as origin — the EOA's substrate account for
 * direct precompile calls, or the contract's substrate-mapped account
 * (`h160ToSubstrate(deployed.addressBytes)`) when a contract forwards the
 * terms.
 */
export async function negotiatePrecompileTerms(
  providerUrl: string,
  owner: { address: string; publicKey: Uint8Array } | ChainSigner,
  { maxBytes, duration, pricePerByte }: { maxBytes: bigint; duration: number; pricePerByte: bigint }
) {
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
      owner: toHex(owner.publicKey),
      maxBytes: BigInt(t.max_bytes),
      duration: Number(t.duration),
      pricePerByte: BigInt(t.price_per_byte),
      validUntil: Number(t.valid_until),
      nonce: BigInt(t.nonce),
      hasReplicaParams: rp != null,
      replicaParams: {
        syncBalance: BigInt(rp?.sync_balance ?? 0),
        minSyncInterval: Number(rp?.min_sync_interval ?? 0),
        syncPrice: 0n,
      },
      hasBucketId: bucket != null,
      bucketId: BigInt(bucket ?? 0),
    },
    signature: signed.signature.startsWith("0x")
      ? signed.signature
      : `0x${signed.signature}`,
  };
}
