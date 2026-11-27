// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it } from 'vitest'
import {
  HISTORY_SLOTS,
  blockBars,
  classifyDelay,
  judgeHealth,
  recordArrival,
  typicalBlockTime,
  type BlockArrival,
} from './chain-health'

function arrivals(...times: number[]): BlockArrival[] {
  return times.map((at, i) => ({ number: i + 1, at }))
}

describe('recordArrival', () => {
  it('appends newer blocks and ignores re-orged ones', () => {
    let history = recordArrival([], 10, 0)
    history = recordArrival(history, 11, 6000)
    history = recordArrival(history, 11, 7000)
    expect(history).toEqual([
      { number: 10, at: 0 },
      { number: 11, at: 6000 },
    ])
  })

  it('keeps one more arrival than the bars', () => {
    let history: BlockArrival[] = []
    for (let n = 1; n <= 100; n++) history = recordArrival(history, n, n * 6000)
    expect(history).toHaveLength(HISTORY_SLOTS + 1)
    expect(blockBars(history)).toHaveLength(HISTORY_SLOTS)
  })
})

describe('typicalBlockTime', () => {
  it('defaults to 6 s with fewer than two blocks', () => {
    expect(typicalBlockTime(arrivals(0))).toBe(6000)
  })

  it('takes the median per-block delay', () => {
    expect(typicalBlockTime(arrivals(0, 6000, 12000, 30000))).toBe(6000)
  })

  it('divides a delay by the blocks skipped', () => {
    expect(typicalBlockTime([{ number: 1, at: 0 }, { number: 3, at: 12000 }])).toBe(6000)
  })
})

describe('classifyDelay', () => {
  it('splits at 1.5 and 3 block times', () => {
    expect(classifyDelay(9000, 6000)).toBe('onTime')
    expect(classifyDelay(9001, 6000)).toBe('late')
    expect(classifyDelay(18001, 6000)).toBe('veryLate')
  })
})

describe('judgeHealth', () => {
  const history = arrivals(0, 6000, 12000)

  it('reports offline on a connection error', () => {
    expect(judgeHealth({ status: 'error', history, now: 12000 })).toBe('offline')
  })

  it('reports connecting until the first block', () => {
    expect(judgeHealth({ status: 'connected', history: [], now: 0 })).toBe('connecting')
  })

  it('reports behind after three block times without a block', () => {
    expect(judgeHealth({ status: 'connected', history, now: 30000 })).toBe('ok')
    expect(judgeHealth({ status: 'connected', history, now: 30001 })).toBe('behind')
  })
})
