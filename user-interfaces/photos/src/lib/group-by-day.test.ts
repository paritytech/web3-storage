// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it } from 'vitest'
import { dayLabel, groupByDay } from './group-by-day'

const at = (year: number, month: number, day: number, hour = 12) => new Date(year, month - 1, day, hour).getTime()

describe('groupByDay', () => {
  it('groups consecutive items by local day', () => {
    const items = [{ modified: at(2026, 11, 25, 18) }, { modified: at(2026, 11, 25, 9) }, { modified: at(2026, 11, 20) }]
    const groups = groupByDay(items)
    expect(groups.map((g) => g.items.length)).toEqual([2, 1])
    expect(groups[0].day).toBe(at(2026, 11, 25, 0))
  })
})

describe('dayLabel', () => {
  const now = at(2026, 11, 25)

  it('names today and yesterday', () => {
    expect(dayLabel(at(2026, 11, 25, 0), now)).toBe('Today')
    expect(dayLabel(at(2026, 11, 24, 0), now)).toBe('Yesterday')
  })

  it('adds the year only for other years', () => {
    expect(dayLabel(at(2026, 3, 1, 0), now)).not.toMatch(/2026/)
    expect(dayLabel(at(2025, 3, 1, 0), now)).toMatch(/2025/)
  })
})
