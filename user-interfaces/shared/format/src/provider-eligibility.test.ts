// SPDX-License-Identifier: GPL-3.0-only

import { describe, it, expect } from 'vitest'
import { providerDisabledReason } from './provider-eligibility'

const match = (partialReason: string, minAgreementBytes = 0n) => ({ partialReason, minAgreementBytes })

describe('providerDisabledReason', () => {
  it('accepts a provider without a partial reason', () => {
    expect(providerDisabledReason(match(''))).toBeNull()
  })

  it('rejects every partial reason the chain reports', () => {
    expect(providerDisabledReason(match('NotAccepting'))).toBe('Not accepting')
    expect(providerDisabledReason(match('InsufficientCapacity'))).toBe('Capacity full')
    expect(providerDisabledReason(match('PriceTooHigh'))).toBe('Price above your max')
    expect(providerDisabledReason(match('DurationMismatch'))).toBe('Duration out of range')
  })

  it('shows the minimum for a request below min_agreement_bytes', () => {
    expect(providerDisabledReason(match('BelowMinAgreementBytes', 2048n))).toBe('Below minimum size (min 2 KB)')
  })

  it('rejects an unknown reason with the raw variant name', () => {
    expect(providerDisabledReason(match('SomeNewReason'))).toBe('SomeNewReason')
  })
})
