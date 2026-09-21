// SPDX-License-Identifier: GPL-3.0-only
//
// List storage providers via the `StorageProvider::find_matching_providers`
// runtime API and annotate each against the user's requested size/duration.
// Mirrors how drive-ui and s3-ui discover providers. Photos only needs the
// fields the create-library picker surfaces (price, capacity, accepting,
// duration bounds) plus the negotiate endpoint resolved from the multiaddr.

import { parseMultiaddrToUrl, toSs58, type ParachainApi } from '@web3-storage/papi'

// Largest u128 — used as the price cap so the runtime API never filters on
// price. Eligibility against the actual size/duration is re-evaluated locally
// by `annotate` as the user edits the form, so `listProviders` asks for the
// full registered set rather than a requirement-filtered slice.
const NO_PRICE_CAP = (1n << 128n) - 1n
// Enough headroom to enumerate every provider on dev/test networks without the
// runtime API's `limit` silently truncating the list.
const PROVIDER_LIMIT = 100

export interface PhotosProvider {
  /** Provider's SS58 account (the bytes32 `provider` arg of `createLibrary`). */
  account: string
  /** Registered libp2p multiaddr (decoded to a string). */
  multiaddr: string
  /** HTTP(S) base URL parsed from the multiaddr, or `null` if not addressable. */
  url: string | null
  pricePerByte: bigint
  acceptingPrimary: boolean
  /** Free capacity per the chain; `undefined` = unlimited, not `0n` ("full"). */
  availableCapacity: bigint | undefined
  maxCapacity: bigint
  minDuration: number
  maxDuration: number
  /** 0-100, computed on-chain by `ProviderStats::reputation`. */
  reputation: number
}

/**
 * Return every registered provider with the fields the picker needs, sorted by
 * free capacity descending. Sourced from the `find_matching_providers` runtime
 * API (which also skips providers that have announced deregistration) rather
 * than a raw storage scan.
 */
export async function listProviders(api: ParachainApi): Promise<PhotosProvider[]> {
  const matches = await api.apis.StorageProviderApi.find_matching_providers(
    { bytes_needed: 0n, min_duration: 0, max_price_per_byte: NO_PRICE_CAP, primary_only: true },
    PROVIDER_LIMIT,
  )
  const decoder = new TextDecoder()

  const providers: PhotosProvider[] = matches.map((match): PhotosProvider => {
    const info = match.info
    const multiaddr = decoder.decode(info.multiaddr)
    const maxCapacity = BigInt(info.max_capacity ?? 0)
    // The chain already computed this; `null` is its "unlimited".
    const availableCapacity =
      info.available_capacity == null ? undefined : BigInt(info.available_capacity)

    return {
      // `match.account` is the SCALE-encoded AccountId (32 raw bytes) — the same
      // key the storage-map scan used to read, and the `provider` arg createLibrary needs.
      account: toSs58(match.account),
      multiaddr,
      url: parseMultiaddrToUrl(multiaddr),
      pricePerByte: info.price_per_byte,
      acceptingPrimary: info.accepting_primary,
      availableCapacity,
      // `max_capacity === 0` means "unlimited" in the pallet.
      maxCapacity,
      minDuration: info.min_duration,
      maxDuration: info.max_duration,
      reputation: info.stats.reputation,
    }
  })

  // Most free capacity first; unlimited (`undefined`) sorts ahead of metered.
  providers.sort((a, b) => {
    if (a.availableCapacity === b.availableCapacity) return 0
    if (a.availableCapacity === undefined) return -1
    if (b.availableCapacity === undefined) return 1
    return b.availableCapacity > a.availableCapacity ? 1 : -1
  })

  return providers
}

export interface ProviderRequirements {
  bytesNeeded: bigint
  durationBlocks: number
}

export interface ProviderEligibility {
  eligible: boolean
  reasons: string[]
}

/**
 * Check whether a provider can serve the requested terms. Returns the reasons
 * it can't, so the picker can disable the row and explain why.
 */
export function annotate(
  provider: PhotosProvider,
  { bytesNeeded, durationBlocks }: ProviderRequirements,
): ProviderEligibility {
  const reasons: string[] = []
  if (!provider.acceptingPrimary) reasons.push('Not accepting')
  if (provider.url === null) reasons.push('No HTTP endpoint')
  // `undefined` free capacity is unlimited; otherwise it must cover the request.
  if (provider.availableCapacity !== undefined && provider.availableCapacity < bytesNeeded) {
    reasons.push('Capacity full')
  }
  if (durationBlocks < provider.minDuration) reasons.push('Duration too short')
  if (provider.maxDuration > 0 && durationBlocks > provider.maxDuration) {
    reasons.push('Duration too long')
  }
  return { eligible: reasons.length === 0, reasons }
}
