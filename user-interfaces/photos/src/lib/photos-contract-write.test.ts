// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it } from 'vitest'
import { classifyDispatchError } from './photos-contract-write'

describe('classifyDispatchError', () => {
  it('maps MaxBytesBelowMinimum to a minimum-size message', () => {
    const e = classifyDispatchError('StorageProvider.MaxBytesBelowMinimum')
    expect(e.kind).toBe('min-size')
    expect(e.message).toContain('minimum agreement size')
  })

  it('maps InvalidMaxBytesRequest to a zero-size message', () => {
    const e = classifyDispatchError('StorageProvider.InvalidMaxBytesRequest')
    expect(e.kind).toBe('min-size')
    expect(e.message).toContain('0 bytes')
  })
})
