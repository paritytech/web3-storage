// SPDX-License-Identifier: GPL-3.0-only

import { describe, it, expect } from 'vitest'
import { providerDisabledReason, type ProviderEligibilityInput } from './provider-eligibility'

const provider: ProviderEligibilityInput = {
  acceptingPrimary: true,
  availableCapacity: undefined,
  minBytes: 0n,
  minDuration: 10,
  maxDuration: 1000,
}

describe('providerDisabledReason', () => {
  it('accepts an eligible provider', () => {
    expect(providerDisabledReason(provider, 1024n, 100)).toBeNull()
  })

  it('rejects a provider that is not accepting primary agreements', () => {
    expect(providerDisabledReason({ ...provider, acceptingPrimary: false }, 1024n, 100)).toBe('Not accepting')
  })

  it('rejects a request below min_bytes and shows the minimum', () => {
    expect(providerDisabledReason({ ...provider, minBytes: 2048n }, 1024n, 100)).toBe(
      'Below minimum size (min 2 KB)',
    )
  })

  it('accepts a request equal to min_bytes', () => {
    expect(providerDisabledReason({ ...provider, minBytes: 1024n }, 1024n, 100)).toBeNull()
  })

  it('re-evaluates for a different size', () => {
    const p = { ...provider, minBytes: 2048n }
    expect(providerDisabledReason(p, 1024n, 100)).not.toBeNull()
    expect(providerDisabledReason(p, 4096n, 100)).toBeNull()
  })

  it('rejects when free capacity is below the request, but not when unlimited', () => {
    expect(providerDisabledReason({ ...provider, availableCapacity: 512n }, 1024n, 100)).toBe('Capacity full')
    expect(providerDisabledReason({ ...provider, availableCapacity: undefined }, 1024n, 100)).toBeNull()
  })

  it('rejects durations outside the bounds and treats max 0 as unbounded', () => {
    expect(providerDisabledReason(provider, 1024n, 5)).toBe('Duration too short')
    expect(providerDisabledReason(provider, 1024n, 2000)).toBe('Duration too long')
    expect(providerDisabledReason({ ...provider, maxDuration: 0 }, 1024n, 2000)).toBeNull()
  })
})
