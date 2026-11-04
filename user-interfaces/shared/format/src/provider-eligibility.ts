// SPDX-License-Identifier: GPL-3.0-only

import { formatBytes } from './index'

/** A provider as scored by the `find_matching_providers` runtime API. */
export interface ProviderEligibilityInput {
  /** The chain's `PartialMatchReason` variant name; `''` when the provider matches. */
  partialReason: string
  minAgreementBytes: bigint
}

/**
 * Why a provider cannot take the requested agreement, or `null` when it can.
 * The chain sets `partialReason` for every requirement the provider fails, so
 * pickers disable a row exactly when this returns text.
 */
export function providerDisabledReason(p: ProviderEligibilityInput): string | null {
  switch (p.partialReason) {
    case '':
      return null
    case 'NotAccepting':
      return 'Not accepting'
    case 'InsufficientCapacity':
      return 'Capacity full'
    case 'BelowMinAgreementBytes':
      return `Below minimum size (min ${formatBytes(p.minAgreementBytes)})`
    case 'PriceTooHigh':
      return 'Price above your max'
    case 'DurationMismatch':
      return 'Duration out of range'
    default:
      return p.partialReason
  }
}
