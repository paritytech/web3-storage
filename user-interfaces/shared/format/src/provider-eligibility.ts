// SPDX-License-Identifier: GPL-3.0-only

import { formatBytes } from './index'

/** The provider fields that decide whether it can take a new primary agreement. */
export interface ProviderEligibilityInput {
  acceptingPrimary: boolean
  /** Free capacity per the chain; `undefined` means unlimited. */
  availableCapacity: bigint | undefined
  minBytes: bigint
  minDuration: number
  /** `0` means no upper bound. */
  maxDuration: number
}

/**
 * Why a provider cannot take an agreement of `capacity` bytes for `duration`
 * blocks, or `null` when it can. Pickers disable the row with this text.
 */
export function providerDisabledReason(
  p: ProviderEligibilityInput,
  capacity: bigint,
  duration: number,
): string | null {
  if (!p.acceptingPrimary) return 'Not accepting'
  if (capacity < p.minBytes) return `Below minimum size (min ${formatBytes(p.minBytes)})`
  if (p.availableCapacity !== undefined && p.availableCapacity < capacity) return 'Capacity full'
  if (duration < p.minDuration) return 'Duration too short'
  if (p.maxDuration > 0 && duration > p.maxDuration) return 'Duration too long'
  return null
}
