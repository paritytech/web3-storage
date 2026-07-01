// SPDX-License-Identifier: GPL-3.0-only
//
// Library State — the M5 "create a library" flow. Lists providers from chain
// and walks a fresh account A→B: map the account, negotiate signed terms with
// the chosen provider (owner = the contract's mapped account), then call
// `createLibrary{value}` on the Photos contract. Mirrors the headless sequence
// in `scripts/photos-flow.ts`, submitting via PAPI `signAndSubmit`.

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'
import type { InjectedPolkadotAccount } from 'polkadot-api/pjs-signer'
import { fromHex, toHex, negotiateProviderTerms } from '@web3-storage/papi'
import { h160ToSubstrate } from '@web3-storage/sdk'
import { getApi } from '@/lib/chain-client'
import type { ResolvedContract } from '@/lib/photos-contract'
import {
  computePaymentAndValue,
  encodeCreateLibrary,
  ensureAccountMapped,
  providerAccountToBytes32,
  submitCreateLibrary,
  toContractTerms,
  type CreateLibraryError,
} from '@/lib/photos-contract-write'
import { listProviders, type PhotosProvider } from '@/lib/photos-providers'

export type CreationStage =
  | 'idle'
  | 'mapping'
  | 'negotiating'
  | 'submitting'
  | 'ready'
  | 'failed'

export interface CreationState {
  stage: CreationStage
  error?: CreateLibraryError
  driveId?: bigint
}

export interface CreateLibraryInput {
  account: InjectedPolkadotAccount
  contract: ResolvedContract
  provider: PhotosProvider
  sizeBytes: bigint
  durationBlocks: number
  name: string
}

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

const providers$ = new BehaviorSubject<PhotosProvider[]>([])
const providersLoading$ = new BehaviorSubject<boolean>(false)
const providersError$ = new BehaviorSubject<string | undefined>(undefined)
const creation$ = new BehaviorSubject<CreationState>({ stage: 'idle' })

// The last attempt's input, so a failed create can be retried without the panel
// re-threading the form values.
let lastInput: CreateLibraryInput | null = null

export const [useProviders] = bind(providers$, [])
export const [useProvidersLoading] = bind(providersLoading$, false)
export const [useProvidersError] = bind(providersError$, undefined)
export const [useCreation] = bind(creation$, { stage: 'idle' })

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

export async function loadProviders(): Promise<void> {
  providersLoading$.next(true)
  providersError$.next(undefined)
  try {
    providers$.next(await listProviders(getApi()))
  } catch (err) {
    providersError$.next(err instanceof Error ? err.message : 'Failed to list providers')
    providers$.next([])
  } finally {
    providersLoading$.next(false)
  }
}

export function resetCreation(): void {
  // Drop the cached attempt too, so a later `retryCreate()` can't fire against a
  // stale account/provider after the user switched context.
  lastInput = null
  creation$.next({ stage: 'idle' })
}

/** Retry the most recent create attempt (re-negotiates fresh terms). */
export async function retryCreate(): Promise<void> {
  if (lastInput) await createLibrary(lastInput)
}

/**
 * Run the full A→B create flow. Sets `creation$` through
 * mapping → negotiating → submitting → ready|failed. On success the caller
 * re-reads `libraryOf` (the page's `refresh` bump) to flip to State B.
 */
export async function createLibrary(input: CreateLibraryInput): Promise<void> {
  lastInput = input
  const { account, contract, provider, sizeBytes, durationBlocks, name } = input
  const api = getApi()
  const signer = account.polkadotSigner

  try {
    // Idempotent: a fresh account must be mapped before any contract write.
    creation$.next({ stage: 'mapping' })
    await ensureAccountMapped(api, signer)

    // Re-read the provider's locked price right before negotiating — the list
    // may be stale, and a drifted price means `PaymentExceedsMax`.
    const info = await api.query.StorageProvider.Providers.getValue(provider.account)
    if (!info) {
      creation$.next({
        stage: 'failed',
        error: { kind: 'negotiate', message: `Provider ${provider.account} is no longer registered.` },
      })
      return
    }
    if (!info.settings.accepting_primary) {
      creation$.next({
        stage: 'failed',
        error: { kind: 'negotiate', message: 'Provider is no longer accepting new agreements.' },
      })
      return
    }
    // Re-check capacity against fresh state too — the cached list (which gated
    // the button) can be stale. `max_capacity === 0` is unlimited.
    const maxCapacity = BigInt(info.settings.max_capacity ?? 0)
    const committedBytes = BigInt(info.committed_bytes ?? 0)
    const availableCapacity = maxCapacity > committedBytes ? maxCapacity - committedBytes : 0n
    if (maxCapacity !== 0n && availableCapacity < sizeBytes) {
      creation$.next({
        stage: 'failed',
        error: {
          kind: 'capacity',
          message: `Provider no longer has room for ${sizeBytes} bytes (free: ${availableCapacity}). Pick another provider or reduce the size.`,
        },
      })
      return
    }
    const pricePerByte = BigInt(info.settings.price_per_byte ?? 0)
    const { value } = computePaymentAndValue(pricePerByte, sizeBytes, durationBlocks)

    // The terms are bound to the *contract's* mapped account, not the user's.
    const contractOwner = h160ToSubstrate(fromHex(contract.address))
    // Negotiate against the provider's *current* endpoint — its registered
    // multiaddr may have changed since the list loaded.
    const multiaddr = new TextDecoder().decode(info.multiaddr)

    creation$.next({ stage: 'negotiating' })
    const negotiated = await negotiateProviderTerms(
      { account: contractOwner.address, multiaddr },
      {
        owner: contractOwner.address,
        max_bytes: sizeBytes,
        duration: durationBlocks,
        price_per_byte: pricePerByte,
        replica_params: null,
        bucket_id: null,
      },
    )
    if (!negotiated.ok) {
      creation$.next({ stage: 'failed', error: { kind: 'negotiate', message: negotiated.error } })
      return
    }

    const { terms, signature } = toContractTerms(contractOwner.publicKey, negotiated.signed)
    const data = encodeCreateLibrary({
      userAccount: toHex(signer.publicKey) as `0x${string}`,
      name,
      provider: providerAccountToBytes32(provider.account),
      terms,
      signature,
    })

    creation$.next({ stage: 'submitting' })
    const result = await submitCreateLibrary(api, signer, fromHex(contract.address), data, { value })
    if (!result.ok) {
      creation$.next({ stage: 'failed', error: result.error })
      return
    }

    creation$.next({ stage: 'ready', driveId: result.driveId })
  } catch (err) {
    creation$.next({ stage: 'failed', error: { kind: 'unknown', message: formatThrown(err) } })
  }
}

/** Render a thrown value (Error or a raw PAPI error object) as readable text. */
function formatThrown(err: unknown): string {
  if (err instanceof Error) return err.message
  let text: string
  try {
    text = JSON.stringify(err)
  } catch {
    text = String(err)
  }
  if (/stale/i.test(text)) {
    return 'The transaction nonce was stale (the chain view lagged). Please try again.'
  }
  return `Transaction failed: ${text}`
}
