// SPDX-License-Identifier: GPL-3.0-only
//
// Drive `pallet_revive` from PAPI (deploy + call + decode contract events) and
// shape provider-signed terms for the drive-registry precompile. A TypeScript
// port of `examples/papi/sc-api.js` (+ the `/negotiate` helper from api.js),
// living in the app so the UI and the headless flow share one implementation.
// viem is used for ABI encode/decode only — no EVM RPC client.

import { ss58Address } from "@polkadot-labs/hdkd-helpers";
import { decodeEventLog, encodeFunctionData, keccak256, type Abi } from "viem";
import { fromHex, toHex } from "@web3-storage/papi";
import type { ParachainApi } from "@web3-storage/papi";

import { submitTx, requireOneEvent, type Signer } from "./papi.js";

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
 * Substrate account `AccountId32Mapper` assigns to an unmapped H160 (e.g. a
 * deployed contract): the 20 address bytes followed by 12 bytes of `0xEE`.
 * Use as the `owner` of negotiated terms when a contract forwards them.
 */
export function h160ToSubstrate(addressBytes: Uint8Array): Owner {
  const publicKey = new Uint8Array(32).fill(0xee);
  publicKey.set(addressBytes, 0);
  return { publicKey, address: ss58Address(publicKey) };
}

/** Forward `AccountId32Mapper`: `keccak256(account_bytes)[12..]` → H160 hex. */
export function substrateToH160(publicKey: Uint8Array): `0x${string}` {
  const hash = fromHex(keccak256(publicKey));
  return toHex(hash.slice(12)) as `0x${string}`;
}

/** POST /negotiate and return the provider's signed terms bundle (raw JSON). */
export async function negotiateTerms(providerUrl: string, request: Record<string, unknown>): Promise<any> {
  const res = await fetch(`${providerUrl}/negotiate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request, (_k, v) => (typeof v === "bigint" ? v.toString() : v)),
  });
  if (!res.ok) {
    throw new Error(`/negotiate failed: ${res.status} ${await res.text().catch(() => "")}`);
  }
  return res.json();
}

/**
 * Negotiate primary terms with `owner` as the bound account, shaped for the
 * precompile's `PrimitiveAgreementTerms` ABI struct. For Photos, `owner` is the
 * contract's substrate-mapped account (`h160ToSubstrate(deployed.addressBytes)`).
 */
export async function negotiatePrecompileTerms(
  providerUrl: string,
  owner: Owner,
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
      owner: toHex(owner.publicKey) as `0x${string}`,
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
    signature: (signed.signature.startsWith("0x") ? signed.signature : `0x${signed.signature}`) as `0x${string}`,
  };
}

// Generous gas/storage defaults — the precompile meters real weight via
// `env.charge`; these just keep the dispatch from being gated on estimation.
const DEFAULT_GAS_LIMIT = { ref_time: 1_000_000_000_000n, proof_size: 4_000_000n };
const DEFAULT_STORAGE_DEPOSIT_LIMIT = 10n ** 18n;

/** Register `signer`'s substrate account with `pallet_revive`. Idempotent. */
export async function ensureAccountMapped(api: ParachainApi, signer: Signer): Promise<void> {
  try {
    await submitTx(api.tx.Revive.map_account(), signer.signer, `Revive.map_account(${signer.seed})`);
  } catch (e: any) {
    const msg = String(e?.message ?? e);
    if (msg.includes("AccountAlreadyMapped") || msg.includes("AlreadyMapped")) return;
    throw e;
  }
}

export interface Deployed {
  address: `0x${string}`;
  addressBytes: Uint8Array;
  events: any[];
}

/** Upload + instantiate in one extrinsic; returns the contract's H160. */
export async function deployContract(
  api: ParachainApi,
  deployer: Signer,
  bytecode: Uint8Array,
  constructorData: Uint8Array = new Uint8Array(),
  { value = 0n, gasLimit = DEFAULT_GAS_LIMIT, storageDepositLimit = DEFAULT_STORAGE_DEPOSIT_LIMIT, salt }:
    { value?: bigint; gasLimit?: typeof DEFAULT_GAS_LIMIT; storageDepositLimit?: bigint; salt?: Uint8Array } = {},
): Promise<Deployed> {
  const tx = api.tx.Revive.instantiate_with_code({
    value,
    weight_limit: gasLimit,
    storage_deposit_limit: storageDepositLimit,
    // This PAPI version maps Vec<u8> → Uint8Array and [u8; N] → 0x-hex string.
    code: bytecode,
    data: constructorData,
    salt: salt ? (toHex(salt) as `0x${string}`) : undefined,
  });
  const result = await submitTx(tx, deployer.signer, "Revive.instantiate_with_code");
  const instantiated: any = requireOneEvent(result.events, api.event.Revive.Instantiated, "Revive.Instantiated");
  const address = instantiated.contract as `0x${string}`; // SizedHex<20> hex string
  return { address, addressBytes: fromHex(address), events: result.events };
}

/** Call a deployed contract with raw ABI-encoded `data`. Returns the in-block result. */
export async function callContract(
  api: ParachainApi,
  signer: Signer,
  contractAddressBytes: Uint8Array,
  data: Uint8Array,
  { value = 0n, gasLimit = DEFAULT_GAS_LIMIT, storageDepositLimit = DEFAULT_STORAGE_DEPOSIT_LIMIT }:
    { value?: bigint; gasLimit?: typeof DEFAULT_GAS_LIMIT; storageDepositLimit?: bigint } = {},
): Promise<any> {
  const tx = api.tx.Revive.call({
    dest: toHex(contractAddressBytes) as `0x${string}`, // SizedHex<20>
    value,
    weight_limit: gasLimit,
    storage_deposit_limit: storageDepositLimit,
    data, // Vec<u8> → Uint8Array
  });
  return submitTx(tx, signer.signer, "Revive.call");
}

/**
 * Decode `Revive.ContractEmitted` events from `contractAddress` (0x H160 hex)
 * against `abi`. In this PAPI version the event payload's `contract`/`topics`
 * are `0x`-hex strings and `data` is a `Uint8Array`.
 */
export function decodeContractEmitted(events: any[], api: ParachainApi, contractAddress: string, abi: Abi): Array<{ eventName: string; args: any }> {
  const decoded: Array<{ eventName: string; args: any }> = [];
  const want = contractAddress.toLowerCase();
  for (const ev of api.event.Revive.ContractEmitted.filter(events) as any[]) {
    const p = ev.payload;
    if (String(p.contract).toLowerCase() !== want) continue;
    try {
      const log = decodeEventLog({
        abi,
        data: toHex(p.data) as `0x${string}`,
        topics: (p.topics ?? []) as [`0x${string}`, ...`0x${string}`[]],
      });
      decoded.push(log as any);
    } catch {
      // event not in this ABI — skip
    }
  }
  return decoded;
}

/** viem `encodeFunctionData` → raw calldata bytes. */
export function encodeCall(abi: Abi, functionName: string, args: unknown[]): Uint8Array {
  return fromHex(encodeFunctionData({ abi, functionName, args } as any));
}
