// SPDX-License-Identifier: Apache-2.0

/**
 * Helpers for driving `pallet_revive` from PAPI: deploy a PolkaVM contract,
 * call a function on it, and read back contract-emitted events. Uses viem
 * only for ABI encoding/decoding (no EVM RPC client) — which is why this
 * module is a separate `@web3-storage/sdk/revive` subpath export: consumers
 * that never touch contracts don't pull viem into their graph.
 *
 * NOTE on PAPI field names: the exact shape of `api.tx.Revive.*` is whatever
 * `papi generate` produces from the runtime metadata. The names below assume
 * snake_case for arguments matching `pallet_revive::Call::call`/`instantiate_with_code`
 * dispatchables on `polkadot-stable2603`. If `papi generate` surfaces a
 * different field name, adjust here.
 */

import { decodeEventLog, encodeFunctionData, keccak256 } from "viem";
import type { Abi } from "viem";

import { asHex, hexToBytes, toHex, type ParachainApi } from "./address.js";
import type { ChainSigner } from "./signers.js";
import { requireOneEvent, submitTx, submitTxFinalized } from "./tx.js";

/**
 * Compute the EVM-side H160 of a substrate account via `AccountId32Mapper`'s
 * forward direction: `keccak256(account_bytes)[12..]`. Use this when you need
 * the `address` view of a substrate-only account (e.g. minting an
 * `address`-keyed token to `//Bob`). The inverse (H160 → `AccountId32`) is
 * `h160ToSubstrate` in `@web3-storage/core` (it needs no viem).
 */
export function substrateToH160(publicKey: Uint8Array | string): string {
  const bytes = publicKey instanceof Uint8Array ? publicKey : hexToBytes(publicKey);
  const hash = hexToBytes(keccak256(bytes));
  // `toHex` (browser-safe, `0x`-prefixed) instead of Node's `Buffer` so this
  // module can be imported from browser bundles too.
  return toHex(hash.slice(12));
}

/**
 * Generous defaults — pallet_revive bounds these at the runtime config level
 * (`RuntimeMemory`, `PVFMemory`). Picking large values means the dispatch
 * isn't gated on tight estimation; the actual weight consumed is metered by
 * the precompile's `env.charge` calls.
 */
export const DEFAULT_GAS_LIMIT = {
  // Cap at ~half a block — block max is 2s × MaxEthExtrinsicWeight (9/10) = 1.8s;
  // staying well under prevents `Invalid::ExhaustsResources` rejection.
  ref_time: 1_000_000_000_000n, // 1s of ref_time
  // Block PoV max is ~5 MiB. `deleteDrive` alone needs ~1 MiB of proof size
  // (touches StorageProvider::MemberBuckets, StorageAgreements, etc.), so we
  // budget generously — under the cap but well above the per-call needs.
  proof_size: 4_000_000n,
};
export const DEFAULT_STORAGE_DEPOSIT_LIMIT = 10n ** 18n; // 10^6 UNIT

/**
 * Register `signer`'s substrate AccountId with `pallet_revive` so it can
 * participate in contract interactions. Idempotent: if the account is already
 * mapped (or is an ETH-native Address20), this becomes a no-op.
 */
export async function ensureAccountMapped(api: ParachainApi, signer: ChainSigner) {
  try {
    await submitTx(
      api.tx.Revive.map_account(),
      signer.signer,
      `Revive.map_account(${signer.seed ?? signer.address})`,
    );
  } catch (e) {
    const msg = String((e as Error)?.message ?? e);
    if (msg.includes("AccountAlreadyMapped") || msg.includes("AlreadyMapped")) {
      return; // idempotent
    }
    throw e;
  }
}

export interface DeployOpts {
  value?: bigint;
  gasLimit?: { ref_time: bigint; proof_size: bigint };
  storageDepositLimit?: bigint;
  salt?: Uint8Array | string;
}

/**
 * Upload + instantiate a contract in one extrinsic. Returns the H160 contract
 * address from the `Instantiated` event (as a hex string with 0x prefix).
 *
 * `value` is in substrate atomic units (not wei). The contract's
 * substrate-mapped account ends up holding it.
 */
export async function deployContract(
  api: ParachainApi,
  deployer: ChainSigner,
  bytecode: Uint8Array,
  constructorData: Uint8Array = new Uint8Array(),
  {
    value = 0n,
    gasLimit = DEFAULT_GAS_LIMIT,
    storageDepositLimit = DEFAULT_STORAGE_DEPOSIT_LIMIT,
    salt,
  }: DeployOpts = {},
) {
  const tx = api.tx.Revive.instantiate_with_code({
    value,
    weight_limit: gasLimit,
    storage_deposit_limit: storageDepositLimit,
    code: bytecode,
    data: constructorData,
    // Option<[u8; 32]>: PAPI surfaces None as `undefined`.
    salt: salt ? asHex(salt) : undefined,
  });

  const result = await submitTx(tx, deployer.signer, "Revive.instantiate_with_code");
  const instantiated = requireOneEvent(
    result.events,
    api.event.Revive.Instantiated,
    "Revive.Instantiated",
  );
  const address = asHex(instantiated.contract);
  return { address, addressBytes: hexToBytes(address), events: result.events };
}

export interface CallOpts {
  value?: bigint;
  gasLimit?: { ref_time: bigint; proof_size: bigint };
  storageDepositLimit?: bigint;
  /**
   * Wait for finalization when a later tx references this call's effect by id
   * (e.g. a precompile that creates a challenge). See submitTxFinalized.
   */
  finalized?: boolean;
}

/**
 * Call a deployed contract. `data` is the raw ABI-encoded calldata (4-byte
 * selector + ABI-encoded args); use {@link encodeCall} to build it.
 *
 * Returns the full submission result (events array + dispatch info) so
 * callers can pluck out pallet events emitted by the precompile's downstream
 * dispatch.
 */
export async function callContract(
  api: ParachainApi,
  signer: ChainSigner,
  contractAddress: Uint8Array | string,
  data: Uint8Array,
  {
    value = 0n,
    gasLimit = DEFAULT_GAS_LIMIT,
    storageDepositLimit = DEFAULT_STORAGE_DEPOSIT_LIMIT,
    finalized = false,
  }: CallOpts = {},
) {
  const tx = api.tx.Revive.call({
    dest: asHex(contractAddress),
    value,
    weight_limit: gasLimit,
    storage_deposit_limit: storageDepositLimit,
    data,
  });
  const submit = finalized ? submitTxFinalized : submitTx;
  return submit(tx, signer.signer, "Revive.call");
}

/**
 * Find `Revive.ContractEmitted` events whose `contract` matches the given
 * H160 and decode them against `abi`. Returns an array of `{ eventName, args }`.
 */
export function decodeContractEmitted(
  events: unknown[],
  _api: ParachainApi,
  contractAddress: Uint8Array | string,
  abi: Abi,
) {
  const wanted = asHex(contractAddress).toLowerCase();
  const decoded: Array<{ eventName: string; args: unknown }> = [];
  for (const ev of events as Array<{ type?: string; value?: { type?: string; value?: any } }>) {
    if (ev.type !== "Revive") continue;
    if (ev.value?.type !== "ContractEmitted") continue;
    const payload = ev.value.value;
    if (asHex(payload.contract).toLowerCase() !== wanted) continue;
    try {
      const log = decodeEventLog({
        abi,
        data: asHex(payload.data),
        topics: (payload.topics || []).map((t: string | Uint8Array) => asHex(t)) as [
          `0x${string}`,
          ...`0x${string}`[],
        ],
      });
      decoded.push(log as unknown as { eventName: string; args: unknown });
    } catch {
      // unknown event (not in this ABI) — skip
    }
  }
  return decoded;
}

/** Convenience wrapper — keeps callers free of viem imports. */
export function encodeCall(abi: Abi, functionName: string, args: unknown[]): Uint8Array {
  return hexToBytes(encodeFunctionData({ abi, functionName, args }));
}
