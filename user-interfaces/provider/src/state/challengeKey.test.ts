// SPDX-License-Identifier: GPL-3.0-only

import { describe, it, expect } from 'vitest'
import { challengeKey } from './challengeKey'

describe('challengeKey', () => {
  it('distinguishes challenges that share an id but differ in deadline', () => {
    // `id` is the per-deadline index, so two challenges at index 0 in different
    // deadline blocks must not collapse to the same key — that collision is what
    // made responding to one challenge light up every row sharing the index.
    const a = challengeKey({ deadline: 100, id: 0 })
    const b = challengeKey({ deadline: 200, id: 0 })
    expect(a).not.toBe(b)
  })

  it('is stable for the same deadline + id', () => {
    expect(challengeKey({ deadline: 100, id: 3 })).toBe(challengeKey({ deadline: 100, id: 3 }))
  })

  it('distinguishes different ids within the same deadline', () => {
    expect(challengeKey({ deadline: 100, id: 0 })).not.toBe(challengeKey({ deadline: 100, id: 1 }))
  })
})
