// SPDX-License-Identifier: GPL-3.0-only
//
// Storage providers for the create-library picker, from the
// `find_matching_providers` runtime API, and a check of each against the
// user's requested size/duration.

import {
  ALL_PROVIDERS,
  byFreeCapacityDesc,
  findMatchingProviders,
  NO_PRICE_CAP,
  type MatchingProviders,
} from '@web3-storage/sdk'
import { parseMultiaddrToUrl, type ParachainApi } from '@web3-storage/papi'
import { formatBytesBinary } from '@web3-storage/format'

export interface PhotosProvider extends MatchingProviders {
  /** HTTP(S) base URL parsed from the multiaddr, or `null` if not addressable. */
  url: string | null
}

/**
 * Every provider that accepts primary agreements, most free capacity first.
 * No size, duration or price filter: `annotate` checks each provider against
 * the form as the user edits it.
 */
export async function listProviders(api: ParachainApi): Promise<PhotosProvider[]> {
  const matches = await findMatchingProviders(
    api,
    { bytesNeeded: 0n, minDuration: 0, maxPricePerByte: NO_PRICE_CAP, primaryOnly: true },
    ALL_PROVIDERS,
  )
  return matches.map((provider) => ({ ...provider, url: parseMultiaddrToUrl(provider.multiaddr) })).sort(byFreeCapacityDesc)
}

export interface ProviderRequirements {
  bytesNeeded: bigint
  durationBlocks: number
}

export interface ProviderEligibility {
  eligible: boolean
  reasons: string[]
}

/** Reason text when `bytesNeeded` is below the provider's minimum agreement size, else `null`. */
export function belowMinimumReason(minAgreementBytes: bigint, bytesNeeded: bigint): string | null {
  return bytesNeeded < minAgreementBytes ? `Below minimum size (min ${formatBytesBinary(minAgreementBytes)})` : null
}

/**
 * Check whether a provider can serve the requested terms. Returns the reasons
 * it can't, so the picker can disable the row and explain why.
 */
export function annotate(
  provider: Pick<
    PhotosProvider,
    'acceptingPrimary' | 'url' | 'minAgreementBytes' | 'availableCapacity' | 'minDuration' | 'maxDuration'
  >,
  { bytesNeeded, durationBlocks }: ProviderRequirements,
): ProviderEligibility {
  const reasons: string[] = []
  if (!provider.acceptingPrimary) reasons.push('Not accepting')
  if (provider.url === null) reasons.push('No HTTP endpoint')
  const belowMinimum = belowMinimumReason(provider.minAgreementBytes, bytesNeeded)
  if (belowMinimum) reasons.push(belowMinimum)
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
