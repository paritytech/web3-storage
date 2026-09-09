// SPDX-License-Identifier: GPL-3.0-only
//
// Read the Photos contract's `libraryOf(user)` unsigned via the `ReviveApi.call`
// runtime API (no signature, no gas) — the browser port of the M1–M3 headless
// read in `scripts/lib/photos.ts`. State detection only; no writes (M5+).

import { decodeFunctionResult, encodeFunctionData, keccak256 } from 'viem'
import { fromHex, toHex, type ParachainApi } from '@web3-storage/papi'
import type { NetworkConfig } from '@web3-storage/network-config'
import { PHOTOS_ABI } from '@/contract/photos-abi'

const LS_CONTRACT_KEY = 'photos.contract'

export interface LibraryState {
  driveId: bigint
  rootCid: `0x${string}`
  exists: boolean
}

/** The contract's deployed H160, and where it came from (for UI messaging). */
export interface ResolvedContract {
  address: `0x${string}`
  source: 'localStorage' | 'env' | 'network'
}

/**
 * Resolve the Photos contract address for the active network. Precedence:
 *   1. `localStorage['photos.contract']` — a dev override (e.g. paste the H160
 *      that `just photos deploy` / `just photos flow` prints), survives reloads.
 *   2. `import.meta.env.VITE_PHOTOS_CONTRACT` — build/dev-time override.
 *   3. `network.photosContract` — the per-network address (unset for now).
 * Returns `null` when none is configured, so the UI can prompt instead of
 * reading against a bogus address.
 */
export function resolveContractAddress(network: NetworkConfig): ResolvedContract | null {
  const fromLs = typeof localStorage !== 'undefined' ? localStorage.getItem(LS_CONTRACT_KEY) : null
  if (isH160(fromLs)) return { address: fromLs, source: 'localStorage' }

  const fromEnv = import.meta.env.VITE_PHOTOS_CONTRACT as string | undefined
  if (isH160(fromEnv)) return { address: fromEnv, source: 'env' }

  if (isH160(network.photosContract)) return { address: network.photosContract, source: 'network' }

  return null
}

/** Persist a dev contract-address override (or clear it when given a falsy value). */
export function setContractOverride(address: string | null): void {
  if (typeof localStorage === 'undefined') return
  if (address && isH160(address)) localStorage.setItem(LS_CONTRACT_KEY, address)
  else localStorage.removeItem(LS_CONTRACT_KEY)
}

function isH160(v: string | null | undefined): v is `0x${string}` {
  return typeof v === 'string' && /^0x[0-9a-fA-F]{40}$/.test(v)
}

/**
 * Forward `AccountId32Mapper`: the substrate account's mapped H160 is
 * `keccak256(account_bytes)[12..]`. The contract keys `libraries` by the
 * caller's substrate-mapped account, so this is how the signed-in account maps
 * to the `libraryOf(user)` key. Mirrors `scripts/lib/contract.ts:substrateToH160`.
 */
export function substrateToH160(publicKey: Uint8Array): `0x${string}` {
  const hash = fromHex(keccak256(publicKey))
  return toHex(hash.slice(12)) as `0x${string}`
}

/**
 * Read `libraryOf(user)` unsigned. `userH160` is the contract's library key
 * (the user's substrate-mapped account); `origin` is any account to attribute
 * the dry-run to. Mirrors `scripts/lib/photos.ts:readLibraryOf`.
 */
export async function readLibraryOf(
  api: ParachainApi,
  contractAddress: `0x${string}`,
  userH160: `0x${string}`,
  origin: string,
): Promise<LibraryState> {
  const inputData = fromHex(
    encodeFunctionData({ abi: PHOTOS_ABI, functionName: 'libraryOf', args: [userH160] }),
  )
  const res: any = await api.apis.ReviveApi.call(
    origin,
    contractAddress, // dest: SizedHex<20>
    0n,
    undefined, // gas_limit: node default (read)
    undefined, // storage_deposit_limit: none
    inputData,
    { at: 'best' },
  )
  if (!res?.result?.success) {
    throw new Error(
      `ReviveApi.call(libraryOf) reverted: ${JSON.stringify(res?.result?.value ?? res)}`,
    )
  }
  const decoded = decodeFunctionResult({
    abi: PHOTOS_ABI,
    functionName: 'libraryOf',
    data: toHex(res.result.value.data) as `0x${string}`,
  }) as unknown as readonly [bigint, `0x${string}`, boolean]
  return { driveId: decoded[0], rootCid: decoded[1], exists: decoded[2] }
}

/** True when a 32-byte rootCid is all-zero (library exists but nothing anchored yet). */
export function isZeroRoot(rootCid: string): boolean {
  return /^0x0*$/.test(rootCid)
}
