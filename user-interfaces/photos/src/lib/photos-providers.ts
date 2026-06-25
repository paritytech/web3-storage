// SPDX-License-Identifier: GPL-3.0-only
//
// List storage providers from `StorageProvider.Providers` and annotate each
// against the user's requested size/duration. A focused port of drive-ui's
// `DriveClient.listAvailableProviders` — Photos only needs the fields the
// create-library picker surfaces (price, capacity, accepting, duration bounds)
// plus the negotiate endpoint resolved from the registered multiaddr.

import { parseMultiaddrToUrl, type ParachainApi } from '@web3-storage/papi'

export interface PhotosProvider {
  /** Provider's SS58 account (the bytes32 `provider` arg of `createLibrary`). */
  account: string
  /** Registered libp2p multiaddr (decoded to a string). */
  multiaddr: string
  /** HTTP(S) base URL parsed from the multiaddr, or `null` if not addressable. */
  url: string | null
  pricePerByte: bigint
  acceptingPrimary: boolean
  availableCapacity: bigint
  maxCapacity: bigint
  minDuration: number
  maxDuration: number
}

/**
 * Walk `StorageProvider.Providers` and return every registered provider with
 * the fields the picker needs, sorted by free capacity descending.
 */
export async function listProviders(api: ParachainApi): Promise<PhotosProvider[]> {
  const entries = await api.query.StorageProvider.Providers.getEntries()
  const decoder = new TextDecoder()
  const providers: PhotosProvider[] = []

  for (const entry of entries) {
    const provider = entry.value
    const account = entry.keyArgs[0] as string
    const settings = provider.settings

    const multiaddr = decoder.decode(provider.multiaddr)
    const maxCapacity = BigInt(settings.max_capacity ?? 0)
    const committedBytes = BigInt(provider.committed_bytes ?? 0)
    const availableCapacity = maxCapacity > committedBytes ? maxCapacity - committedBytes : 0n

    providers.push({
      account,
      multiaddr,
      url: parseMultiaddrToUrl(multiaddr),
      pricePerByte: BigInt(settings.price_per_byte ?? 0),
      acceptingPrimary: settings.accepting_primary ?? false,
      availableCapacity,
      // `max_capacity === 0` means "unlimited" in the pallet.
      maxCapacity,
      minDuration: settings.min_duration ?? 0,
      maxDuration: settings.max_duration ?? 0,
    })
  }

  providers.sort((a, b) => {
    if (b.availableCapacity > a.availableCapacity) return 1
    if (b.availableCapacity < a.availableCapacity) return -1
    return 0
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
  // `max_capacity === 0` is unlimited; otherwise it must cover the request.
  if (provider.maxCapacity !== 0n && provider.availableCapacity < bytesNeeded) {
    reasons.push('Capacity full')
  }
  if (durationBlocks < provider.minDuration) reasons.push('Duration too short')
  if (provider.maxDuration > 0 && durationBlocks > provider.maxDuration) {
    reasons.push('Duration too long')
  }
  return { eligible: reasons.length === 0, reasons }
}
