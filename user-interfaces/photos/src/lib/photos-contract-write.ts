// SPDX-License-Identifier: GPL-3.0-only
//
// Write path for the Photos contract: map the signer's account, encode
// `createLibrary`, and submit it through `pallet_revive` via PAPI's
// `signAndSubmit`. The browser port of the M1 create sequence in
// `scripts/photos-flow.ts` (+ `scripts/lib/contract.ts`), but submitting with
// `tx.signAndSubmit(signer)` instead of the script-only `submitTx` loop. viem is
// used for ABI encode/decode only — no EVM RPC client.

import { ss58Decode } from '@polkadot-labs/hdkd-helpers'
import { decodeEventLog, encodeFunctionData } from 'viem'
import type { PolkadotSigner } from 'polkadot-api'
import { fromHex, toHex, toSs58, type ParachainApi, type SignedTerms } from '@web3-storage/papi'
import { getClient } from '@/lib/chain-client'
import { PHOTOS_ABI } from '@/contract/photos-abi'

/** Token base unit (12 decimals, like Polkadot). */
export const UNIT = 10n ** 12n

// Generous gas/storage defaults — the precompile meters real weight via
// `env.charge`; these just keep the dispatch from being gated on estimation.
// Mirrors `scripts/lib/contract.ts`.
const DEFAULT_GAS_LIMIT = { ref_time: 1_000_000_000_000n, proof_size: 4_000_000n }
const DEFAULT_STORAGE_DEPOSIT_LIMIT = 10n ** 18n

/** `IDriveRegistry.PrimitiveAgreementTerms`, shaped for viem ABI encoding. */
export interface PrimitiveAgreementTerms {
  owner: `0x${string}`
  maxBytes: bigint
  duration: number
  pricePerByte: bigint
  validUntil: number
  nonce: bigint
  hasBucketId: boolean
  bucketId: bigint
  hasReplicaParams: boolean
  replicaParams: { syncBalance: bigint; minSyncInterval: number; syncPrice: bigint }
}

/** Provider-signed terms reshaped for the contract ABI (camelCase, owner as bytes32). */
export interface ContractSignedTerms {
  terms: PrimitiveAgreementTerms
  signature: `0x${string}`
}

/** Owner-shaped value the negotiate helper needs (a mapped account). */
export interface MappedOwner {
  publicKey: Uint8Array
  address: string
}

/**
 * Substrate account `AccountId32Mapper` assigns to an unmapped H160 (e.g. a
 * deployed contract): the 20 address bytes followed by 12 bytes of `0xEE`. Use
 * as the `owner` of negotiated terms when the contract forwards them. Mirrors
 * `scripts/lib/contract.ts:h160ToSubstrate`.
 */
export function h160ToSubstrate(addressBytes: Uint8Array): MappedOwner {
  const publicKey = new Uint8Array(32).fill(0xee)
  publicKey.set(addressBytes, 0)
  return { publicKey, address: toSs58(publicKey) }
}

/** Decode a provider's SS58 account to its raw 32-byte public key, as bytes32 hex. */
export function providerAccountToBytes32(ss58: string): `0x${string}` {
  const [bytes] = ss58Decode(ss58)
  return toHex(bytes) as `0x${string}`
}

/** Compute the agreement `payment` and the buffered `value` sent with `createLibrary`. */
export function computePaymentAndValue(
  pricePerByte: bigint,
  sizeBytes: bigint,
  durationBlocks: number,
): { payment: bigint; value: bigint } {
  const payment = pricePerByte * sizeBytes * BigInt(durationBlocks)
  // Generous buffer; the unused reserve stays in the contract in v1.
  const value = payment * 2n + UNIT
  return { payment, value }
}

/**
 * Reshape the provider's `/negotiate` response (snake_case, owner as SS58) into
 * the contract's `PrimitiveAgreementTerms` tuple (camelCase, owner as bytes32).
 * `ownerPublicKey` is the contract's mapped account — the bound owner of the
 * terms. Mirrors `scripts/lib/contract.ts:negotiatePrecompileTerms`.
 */
export function toContractTerms(
  ownerPublicKey: Uint8Array,
  signed: SignedTerms,
): ContractSignedTerms {
  const t = signed.terms
  const rp = (t.replica_params ?? null) as {
    sync_balance?: number | bigint
    min_sync_interval?: number
    sync_price?: number | bigint
  } | null
  const bucket = t.bucket_id
  return {
    terms: {
      owner: toHex(ownerPublicKey) as `0x${string}`,
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
    signature: (signed.signature.startsWith('0x')
      ? signed.signature
      : `0x${signed.signature}`) as `0x${string}`,
  }
}

/** Arguments for `createLibrary`, in ABI order. */
export interface CreateLibraryArgs {
  userAccount: `0x${string}`
  name: string
  provider: `0x${string}`
  terms: PrimitiveAgreementTerms
  signature: `0x${string}`
}

/** viem `encodeFunctionData(createLibrary)` → raw calldata bytes. */
export function encodeCreateLibrary(args: CreateLibraryArgs): Uint8Array {
  return fromHex(
    encodeFunctionData({
      abi: PHOTOS_ABI,
      functionName: 'createLibrary',
      args: [args.userAccount, args.name, args.provider, args.terms, args.signature],
    }),
  )
}

/** Stringify a PAPI dispatch error, coercing bigints (mirrors drive-ui `submit()`). */
function stringifyDispatchError(dispatchError: unknown): string {
  return JSON.stringify(dispatchError, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))
}

// A submission can be rejected as `Stale` (nonce too low) when two same-signer
// txs go out back-to-back faster than the client's chain view advances — here,
// `map_account` immediately followed by the `createLibrary` call. PAPI rejects
// with `{ type: 'Invalid', value: { type: 'Stale' } }`. Mirror the headless
// flow (`scripts/lib/papi.ts`): wait for a fresh block and re-sign.
const STALE_MAX_ATTEMPTS = 4

function isStaleError(err: unknown): boolean {
  let text: string
  if (err instanceof Error) text = err.message
  else {
    try {
      text = JSON.stringify(err)
    } catch {
      text = String(err)
    }
  }
  return /stale/i.test(text)
}

/**
 * Resolve once the chain produces a new best block past the one seen when this
 * was called (best, not finalized — the nonce-collision window closes at
 * inclusion, matching the headless flow's in-block resolution; waiting for
 * finalization would add ~12-18s per retry for no benefit). Resolves on a
 * bounded timeout so a stalled chain can't hang the retry indefinitely.
 */
function waitForNextBlock(timeoutMs = 12_000): Promise<void> {
  return new Promise((resolve) => {
    const client = getClient()
    if (!client) {
      setTimeout(resolve, 6_000)
      return
    }
    let initial: number | null = null
    let unsubscribe = () => {}
    const timer = setTimeout(() => {
      unsubscribe()
      resolve()
    }, timeoutMs)
    const sub = client.bestBlocks$.subscribe({
      next: (blocks) => {
        const tip = blocks[0]?.number
        if (tip == null) return
        if (initial === null) {
          initial = tip
          return
        }
        if (tip > initial) {
          clearTimeout(timer)
          unsubscribe()
          resolve()
        }
      },
      error: () => {
        clearTimeout(timer)
        unsubscribe()
        resolve()
      },
    })
    unsubscribe = () => sub.unsubscribe()
  })
}

/** `signAndSubmit` with a bounded retry on stale-nonce rejections. */
async function signAndSubmitWithRetry<T>(
  tx: { signAndSubmit: (signer: PolkadotSigner) => Promise<T> },
  signer: PolkadotSigner,
): Promise<T> {
  for (let attempt = 1; ; attempt++) {
    try {
      return await tx.signAndSubmit(signer)
    } catch (err) {
      if (attempt >= STALE_MAX_ATTEMPTS || !isStaleError(err)) throw err
      await waitForNextBlock()
    }
  }
}

export type CreateLibraryErrorKind =
  | 'payment-exceeds-max'
  | 'terms-expired'
  | 'terms-reused'
  | 'bad-signature'
  | 'already-exists'
  | 'capacity'
  | 'reverted'
  | 'negotiate'
  | 'unknown'

export interface CreateLibraryError {
  kind: CreateLibraryErrorKind
  message: string
}

/**
 * The honest message for a contract-level revert we can't attribute precisely.
 * A revert inside `Revive.call` (the `createLibrary` precompile call failing, or
 * Photos.sol's own `require`) does not fail the extrinsic and carries no
 * reliable reason string back to the client, so we list the likely causes.
 */
export function contractRevertedError(): CreateLibraryError {
  return {
    kind: 'reverted',
    message:
      'The contract rejected createLibrary. Likely causes: the payment was too low for the provider’s price (pick a cheaper provider or reduce size/duration), the signed terms expired (try again), or a library already exists for this account.',
  }
}

/**
 * Best-effort map of a failed-dispatch error to a UI cause, for the case where a
 * revert *does* surface as `result.ok === false` (version-dependent). The inner
 * agreement errors are often collapsed into a generic Revive trap, so an
 * unmatched error falls through to `unknown` — the no-event guard in
 * `submitCreateLibrary` is the primary detector of a reverted call.
 */
export function classifyDispatchError(dispatchError: unknown): CreateLibraryError {
  const raw = stringifyDispatchError(dispatchError)
  if (raw.includes('PaymentExceedsMax')) {
    return {
      kind: 'payment-exceeds-max',
      message:
        'The payment was rejected as too low for these terms. Lower the size/duration or pick a cheaper provider, then try again.',
    }
  }
  if (raw.includes('TermsExpired') || raw.includes('AgreementExpired')) {
    return {
      kind: 'terms-expired',
      message: 'The provider-signed terms expired before the transaction landed. Try again to re-negotiate.',
    }
  }
  if (raw.includes('NonceAlreadyUsed')) {
    return {
      kind: 'terms-reused',
      message: 'The signed terms were already used. Try again to re-negotiate fresh terms.',
    }
  }
  if (raw.includes('InvalidSignature') || raw.includes('BadSignature')) {
    return {
      kind: 'bad-signature',
      message: 'The provider returned an invalid signature. Try another provider.',
    }
  }
  if (raw.includes('library exists')) {
    return { kind: 'already-exists', message: 'A library already exists for this account.' }
  }
  return { kind: 'unknown', message: `Transaction failed on-chain: ${raw}` }
}

/**
 * Register `signer`'s substrate account with `pallet_revive`. Idempotent: a
 * re-map of an already-mapped account is treated as success. Mirrors
 * `scripts/lib/contract.ts:ensureAccountMapped`.
 */
export async function ensureAccountMapped(api: ParachainApi, signer: PolkadotSigner): Promise<void> {
  const result = await signAndSubmitWithRetry(api.tx.Revive.map_account(), signer)
  if (result.ok) return
  const raw = stringifyDispatchError(result.dispatchError)
  if (raw.includes('AccountAlreadyMapped') || raw.includes('AlreadyMapped')) return
  throw new Error(`Revive.map_account failed: ${raw}`)
}

export type SubmitCreateLibraryResult =
  | { ok: true; driveId?: bigint }
  | { ok: false; error: CreateLibraryError }

/**
 * Submit `createLibrary` calldata to the deployed contract via `Revive.call`,
 * attaching `value` (the buffered payment). Returns a discriminated result so
 * the caller can surface a classified error inline rather than catching throws.
 *
 * A contract revert (the `createDrive` precompile rejecting the payment/terms,
 * or Photos.sol's `require`) does NOT fail the `Revive.call` extrinsic —
 * `result.ok` stays true with no drive created. So success is confirmed the way
 * the headless flow asserts it (`scripts/photos-flow.ts`): the on-chain
 * `DriveRegistry.DriveCreated` event (and the contract's `LibraryCreated` log).
 * Their absence means the call reverted.
 */
export async function submitCreateLibrary(
  api: ParachainApi,
  signer: PolkadotSigner,
  contractAddressBytes: Uint8Array,
  data: Uint8Array,
  { value }: { value: bigint },
): Promise<SubmitCreateLibraryResult> {
  const tx = api.tx.Revive.call({
    dest: toHex(contractAddressBytes) as `0x${string}`, // SizedHex<20>
    value,
    weight_limit: DEFAULT_GAS_LIMIT,
    storage_deposit_limit: DEFAULT_STORAGE_DEPOSIT_LIMIT,
    data, // Vec<u8> → Uint8Array
  })
  const result = await signAndSubmitWithRetry(tx, signer)
  if (!result.ok) {
    return { ok: false, error: classifyDispatchError(result.dispatchError) }
  }
  const driveId = driveIdFromEvents(result.events, api, toHex(contractAddressBytes))
  const created =
    driveId !== undefined || api.event.DriveRegistry.DriveCreated.filter(result.events).length > 0
  if (!created) {
    return { ok: false, error: contractRevertedError() }
  }
  return { ok: true, driveId }
}

/**
 * Decode the `driveId` from the contract's `LibraryCreated` log in a successful
 * `createLibrary` result, or `undefined` if it can't be found. Best-effort: the
 * UI flips to State B from the unsigned `libraryOf` re-read regardless.
 */
export function driveIdFromEvents(
  events: unknown[],
  api: ParachainApi,
  contractAddress: string,
): bigint | undefined {
  const want = contractAddress.toLowerCase()
  for (const ev of api.event.Revive.ContractEmitted.filter(events as never[]) as Array<{
    payload: { contract: string; data: Uint8Array; topics?: `0x${string}`[] }
  }>) {
    const p = ev.payload
    if (String(p.contract).toLowerCase() !== want) continue
    try {
      const log = decodeEventLog({
        abi: PHOTOS_ABI,
        data: toHex(p.data) as `0x${string}`,
        topics: (p.topics ?? []) as [`0x${string}`, ...`0x${string}`[]],
      }) as { eventName: string; args?: { driveId?: bigint } }
      if (log.eventName === 'LibraryCreated' && log.args?.driveId != null) return log.args.driveId
    } catch {
      // not in this ABI — skip
    }
  }
  return undefined
}
