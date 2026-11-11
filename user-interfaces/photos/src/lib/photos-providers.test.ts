// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it } from 'vitest'
import { annotate, belowMinimumReason } from './photos-providers'

const provider: Parameters<typeof annotate>[0] = {
  url: 'http://127.0.0.1:3333',
  acceptingPrimary: true,
  availableCapacity: undefined,
  minAgreementBytes: 0n,
  minDuration: 10,
  maxDuration: 1000,
}

const want = { bytesNeeded: 1024n, durationBlocks: 100 }

describe('annotate', () => {
  it('accepts an eligible provider', () => {
    expect(annotate(provider, want)).toEqual({ eligible: true, reasons: [] })
  })

  it('rejects a size below min_agreement_bytes and names the minimum', () => {
    const r = annotate({ ...provider, minAgreementBytes: 2048n }, want)
    expect(r.eligible).toBe(false)
    expect(r.reasons).toEqual(['Below minimum size (min 2 KiB)'])
  })

  it('re-evaluates when the size changes', () => {
    const p = { ...provider, minAgreementBytes: 2048n }
    expect(annotate(p, { ...want, bytesNeeded: 4096n }).eligible).toBe(true)
  })
})

describe('belowMinimumReason', () => {
  it('is null at or above the minimum', () => {
    expect(belowMinimumReason(1024n, 1024n)).toBeNull()
    expect(belowMinimumReason(0n, 1n)).toBeNull()
  })

  it('names the minimum below it', () => {
    expect(belowMinimumReason(1024n, 1023n)).toContain('1 KiB')
  })
})
