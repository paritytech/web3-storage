/**
 * Helpers for driving `pallet_revive` from PAPI: deploy a PolkaVM contract,
 * call a function on it, and read back contract-emitted events. Uses viem
 * only for ABI encoding/decoding (no EVM RPC client).
 *
 * NOTE on PAPI field names: the exact shape of `api.tx.Revive.*` is whatever
 * `papi:generate` produces from the runtime metadata. The names below assume
 * snake_case for arguments matching `pallet_revive::Call::call`/`instantiate_with_code`
 * dispatchables on `polkadot-stable2603`. If `papi:generate` surfaces a
 * different field name, adjust here.
 */

import { Binary } from "@polkadot-api/substrate-bindings";
import { ss58Address } from "@polkadot-labs/hdkd-helpers";
import { decodeEventLog, encodeFunctionData, keccak256 } from "viem";

import { negotiateTerms } from "./api.js";
import {
  hexToBytes,
  requireOneEvent,
  submitTx,
  submitTxFinalized,
  toHex,
} from "./common.js";

/**
 * Compute the EVM-side H160 of a substrate account via `AccountId32Mapper`'s
 * forward direction: `keccak256(account_bytes)[12..]`. Use this when you need
 * the `address` view of a substrate-only account (e.g. minting an
 * `address`-keyed token to `//Bob`).
 */
export function substrateToH160(publicKey) {
  const bytes = publicKey instanceof Uint8Array ? publicKey : hexToBytes(publicKey);
  const hash = hexToBytes(keccak256(bytes));
  return "0x" + Buffer.from(hash.slice(12)).toString("hex");
}

/**
 * Substrate account `AccountId32Mapper` assigns to an unmapped H160 (e.g. a
 * deployed contract): the 20 address bytes followed by 12 bytes of `0xEE`.
 * Returns a signer-shaped `{ publicKey, address }` so it can be passed as
 * the `owner` of negotiated agreement terms.
 */
export function h160ToSubstrate(addressBytes) {
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
export async function negotiatePrecompileTerms(providerUrl, owner, { maxBytes, duration, pricePerByte }) {
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
        syncPrice: BigInt(rp?.sync_price ?? 0),
      },
      hasBucketId: bucket != null,
      bucketId: BigInt(bucket ?? 0),
    },
    signature: signed.signature.startsWith("0x")
      ? signed.signature
      : `0x${signed.signature}`,
  };
}

/**
 * Generous defaults — pallet_revive bounds these at the runtime config level
 * (`RuntimeMemory`, `PVFMemory`). Picking large values means the dispatch
 * isn't gated on tight estimation; the actual weight consumed is metered by
 * the precompile's `env.charge` calls.
 */
const DEFAULT_GAS_LIMIT = {
  // Cap at ~half a block — block max is 2s × MaxEthExtrinsicWeight (9/10) = 1.8s;
  // staying well under prevents `Invalid::ExhaustsResources` rejection.
  ref_time: 1_000_000_000_000n, // 1s of ref_time
  // Block PoV max is ~5 MiB. `deleteDrive` alone needs ~1 MiB of proof size
  // (touches StorageProvider::MemberBuckets, StorageAgreements, etc.), so we
  // budget generously — under the cap but well above the per-call needs.
  proof_size: 4_000_000n,
};
const DEFAULT_STORAGE_DEPOSIT_LIMIT = 10n ** 18n; // 10^6 UNIT

/**
 * Register `signer`'s substrate AccountId with `pallet_revive` so it can
 * participate in contract interactions. Idempotent: if the account is already
 * mapped (or is an ETH-native Address20), this becomes a no-op.
 */
export async function ensureAccountMapped(api, signer) {
  try {
    await submitTx(
      api.tx.Revive.map_account(),
      signer.signer,
      `Revive.map_account(${signer.seed})`
    );
  } catch (e) {
    const msg = String(e?.message ?? e);
    if (msg.includes("AccountAlreadyMapped") || msg.includes("AlreadyMapped")) {
      return; // idempotent
    }
    throw e;
  }
}

/**
 * Upload + instantiate a contract in one extrinsic. Returns the H160 contract
 * address from the `Instantiated` event (as a hex string with 0x prefix).
 *
 * `value` is in substrate atomic units (not wei). The contract's substrate-mapped
 * account ends up holding it.
 */
export async function deployContract(
  api,
  deployer,
  bytecode,
  constructorData = new Uint8Array(),
  { value = 0n, gasLimit = DEFAULT_GAS_LIMIT, storageDepositLimit = DEFAULT_STORAGE_DEPOSIT_LIMIT, salt } = {}
) {
  const tx = api.tx.Revive.instantiate_with_code({
    value,
    weight_limit: gasLimit,
    storage_deposit_limit: storageDepositLimit,
    code: Binary.fromBytes(bytecode),
    data: Binary.fromBytes(constructorData),
    // Option<[u8; 32]>: PAPI surfaces None as `undefined`.
    salt: salt ? Binary.fromBytes(salt) : undefined,
  });

  const result = await submitTx(tx, deployer.signer, "Revive.instantiate_with_code");
  const instantiated = requireOneEvent(
    result.events,
    api.event.Revive.Instantiated,
    "Revive.Instantiated"
  );
  const addrBytes = instantiated.contract.asBytes
    ? instantiated.contract.asBytes()
    : instantiated.contract;
  return { address: toHex(addrBytes), addressBytes: addrBytes, events: result.events };
}

/**
 * Call a deployed contract. `data` is the raw ABI-encoded calldata (4-byte
 * selector + ABI-encoded args); use viem's `encodeFunctionData` to build it.
 *
 * Returns the full `result` (events array + dispatchInfo) so callers can
 * pluck out pallet events emitted by the precompile's downstream dispatch.
 */
export async function callContract(
  api,
  signer,
  contractAddressBytes,
  data,
  {
    value = 0n,
    gasLimit = DEFAULT_GAS_LIMIT,
    storageDepositLimit = DEFAULT_STORAGE_DEPOSIT_LIMIT,
    // Wait for finalization when a later tx references this call's effect by id
    // (e.g. a precompile that creates a challenge). See submitTxFinalized.
    finalized = false,
  } = {}
) {
  const tx = api.tx.Revive.call({
    dest: Binary.fromBytes(contractAddressBytes),
    value,
    weight_limit: gasLimit,
    storage_deposit_limit: storageDepositLimit,
    data: Binary.fromBytes(data),
  });
  const submit = finalized ? submitTxFinalized : submitTx;
  return submit(tx, signer.signer, "Revive.call");
}

/**
 * Find `Revive.ContractEmitted` events whose `contract` matches the given
 * H160 and decode them against `abi`. Returns an array of `{ eventName, args }`.
 */
export function decodeContractEmitted(events, api, contractAddressBytes, abi) {
  const decoded = [];
  for (const ev of events) {
    if (ev.type !== "Revive") continue;
    if (ev.value?.type !== "ContractEmitted") continue;
    const payload = ev.value.value;
    const emitterBytes = payload.contract.asBytes
      ? payload.contract.asBytes()
      : payload.contract;
    if (!bytesEq(emitterBytes, contractAddressBytes)) continue;
    const dataBytes = payload.data.asBytes ? payload.data.asBytes() : payload.data;
    const topicsBytes = (payload.topics || []).map((t) =>
      t.asBytes ? t.asBytes() : t
    );
    try {
      const log = decodeEventLog({
        abi,
        data: toHex(dataBytes),
        topics: topicsBytes.map(toHex),
      });
      decoded.push(log);
    } catch (_) {
      // unknown event (not in this ABI) — skip
    }
  }
  return decoded;
}

function bytesEq(a, b) {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/** Convenience wrappers — keep callers free of viem imports. */
export function encodeCall(abi, functionName, args) {
  return Uint8Array.from(
    Buffer.from(
      encodeFunctionData({ abi, functionName, args }).slice(2),
      "hex"
    )
  );
}
