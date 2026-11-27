// SPDX-License-Identifier: GPL-3.0-only
//
// Chain health from best-block arrival times: the recent blocks as bars
// coloured by delay, the typical block time, and an overall state. Pure
// functions, so the chain status popover and the tests share them.

/** How many recent best blocks the status popover shows. */
export const HISTORY_SLOTS = 48

/** The block time assumed until two blocks have arrived. */
const DEFAULT_BLOCK_TIME_MS = 6_000

/** A best block and when this app first saw it. */
export interface BlockArrival {
  number: number
  /** Milliseconds since the Unix epoch. */
  at: number
}

export type BarHealth = 'onTime' | 'late' | 'veryLate'

/** One bar of the block strip. */
export interface BlockBar {
  number: number
  /** Time since the previous bar's block, per block when blocks were skipped. */
  delayMs: number
  health: BarHealth
}

export type ChainHealth = 'connecting' | 'ok' | 'behind' | 'offline'

/**
 * Append a new best block to `history`, keeping the last `HISTORY_SLOTS + 1`
 * arrivals (one more than the bars, since a bar needs the previous arrival).
 * A block number at or below the latest one (a re-org) is ignored.
 */
export function recordArrival(history: BlockArrival[], number: number, at: number): BlockArrival[] {
  const last = history.at(-1)
  if (last && number <= last.number) return history
  return [...history, { number, at }].slice(-(HISTORY_SLOTS + 1))
}

/** The time per block between each arrival and the one before it. */
function blockIntervals(history: BlockArrival[]): { number: number; delayMs: number }[] {
  return history.slice(1).map((arrival, i) => {
    const previous = history[i]
    return { number: arrival.number, delayMs: (arrival.at - previous.at) / (arrival.number - previous.number) }
  })
}

/** The median time per block, or the default until two blocks have arrived. */
export function typicalBlockTime(history: BlockArrival[]): number {
  const delays = blockIntervals(history)
    .map((interval) => interval.delayMs)
    .sort((a, b) => a - b)
  if (delays.length === 0) return DEFAULT_BLOCK_TIME_MS
  const middle = Math.floor(delays.length / 2)
  return delays.length % 2 === 1 ? delays[middle] : (delays[middle - 1] + delays[middle]) / 2
}

/** Classify a delay against the typical block time. */
export function classifyDelay(delayMs: number, blockTimeMs: number): BarHealth {
  if (delayMs <= blockTimeMs * 1.5) return 'onTime'
  if (delayMs <= blockTimeMs * 3) return 'late'
  return 'veryLate'
}

/** The strip's bars, oldest first. */
export function blockBars(history: BlockArrival[]): BlockBar[] {
  const blockTime = typicalBlockTime(history)
  return blockIntervals(history).map(({ number, delayMs }) => ({
    number,
    delayMs,
    health: classifyDelay(delayMs, blockTime),
  }))
}

/**
 * The chain's state: `offline` when the connection failed, `connecting` until
 * the first block arrives, `behind` when no block arrived for three block
 * times, `ok` otherwise.
 */
export function judgeHealth(input: {
  status: 'disconnected' | 'connecting' | 'connected' | 'error'
  history: BlockArrival[]
  now: number
}): ChainHealth {
  if (input.status === 'error') return 'offline'
  const last = input.history.at(-1)
  if (input.status !== 'connected' || !last) return 'connecting'
  return input.now - last.at > typicalBlockTime(input.history) * 3 ? 'behind' : 'ok'
}
