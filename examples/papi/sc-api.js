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
import { decodeEventLog, encodeFunctionData, keccak256 } from "viem";

import {
  hexToBytes,
  READ_OPTS,
  requireOneEvent,
  submitTx,
  submitTxFinalized,
  toHex,
} from "./common.js";

/**
 * Read `signer`'s nonce from the BEST block and return it as PAPI `txOpts`.
 *
 * A fresh PAPI client's internal nonce view can trail the chain when
 * back-to-back CI tests reuse the same signing account, so its first submit is
 * rejected as `Invalid::Stale` (nonce too low). Reading the nonce ourselves
 * from the best block (a finalized read lags ~6 blocks and is itself stale)
 * and passing it explicitly sidesteps that cache. Contract calls await
 * in-block before returning, so the next read already sees the bump.
 */
async function bestBlockNonceOpts(api, signer) {
  const account = await api.query.System.Account.getValue(
    signer.address,
    READ_OPTS
  );
  return { nonce: account.nonce };
}

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
      `Revive.map_account(${signer.seed})`,
      undefined,
      await bestBlockNonceOpts(api, signer)
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

  const result = await submitTx(
    tx,
    deployer.signer,
    "Revive.instantiate_with_code",
    undefined,
    await bestBlockNonceOpts(api, deployer)
  );
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
  return submit(
    tx,
    signer.signer,
    "Revive.call",
    undefined,
    await bestBlockNonceOpts(api, signer)
  );
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
