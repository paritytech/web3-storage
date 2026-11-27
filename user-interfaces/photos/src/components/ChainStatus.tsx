// SPDX-License-Identifier: GPL-3.0-only
//
// The header's chain status: a dot coloured by the chain's health and the best
// block number. Clicking it opens a popover with the runtime, the best and
// finalized blocks, the typical block time and the last blocks as bars
// coloured by how late each one arrived.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useBestHistory, useConnectionError, useConnectionStatus, useFinalizedNumber, useRuntime } from '@/state/chain.state'
import {
  HISTORY_SLOTS,
  blockBars,
  judgeHealth,
  typicalBlockTime,
  type BarHealth,
  type ChainHealth,
} from '@/lib/chain-health'
import { formatBlockNumber } from '@web3-storage/format'
import { cn } from '@/utils/cn'
import { useClickOutside } from '@/lib/use-click-outside'

const HEALTH: Record<ChainHealth, { word: string; dot: string; pulse: boolean }> = {
  connecting: { word: 'Connecting', dot: 'bg-indicator', pulse: true },
  ok: { word: 'Connected', dot: 'bg-success', pulse: false },
  behind: { word: 'Behind', dot: 'bg-warning', pulse: false },
  offline: { word: 'Offline', dot: 'bg-error', pulse: false },
}

const BAR: Record<BarHealth, { word: string; color: string }> = {
  onTime: { word: 'on time', color: 'bg-success' },
  late: { word: 'late', color: 'bg-warning' },
  veryLate: { word: 'very late', color: 'bg-error' },
}

export function ChainStatus() {
  const status = useConnectionStatus()
  const error = useConnectionError()
  const runtime = useRuntime()
  const finalized = useFinalizedNumber()
  const history = useBestHistory()
  const now = useNow(1000)
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  useClickOutside(containerRef, open, () => setOpen(false))

  const health = judgeHealth({ status, history, now })
  const best = history.at(-1)
  const bars = useMemo(() => blockBars(history), [history])
  const blockTime = typicalBlockTime(history)
  const look = HEALTH[health]

  return (
    <div ref={containerRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-label={`Network: ${look.word}`}
        className="flex cursor-pointer items-center gap-2 rounded-full px-3 py-1.5 text-sm text-secondary transition-colors hover:bg-selection-container-hover hover:text-secondary-hover"
        data-testid="chain-status"
      >
        <Dot className={look.dot} pulse={look.pulse} />
        {best && <span className="tabular-nums" data-testid="block-number">{formatBlockNumber(best.number)}</span>}
      </button>

      {open && (
        <div
          className="absolute right-0 top-full z-50 mt-2 w-80 rounded-container bg-surface-container p-4 shadow-2"
          data-testid="chain-status-popover"
        >
          <div className="flex items-center justify-between">
            <h2 className="font-semibold leading-tight text-primary">Network</h2>
            <span className="flex items-center gap-2 text-sm text-secondary">
              <Dot className={look.dot} pulse={look.pulse} />
              {look.word}
            </span>
          </div>

          {health === 'offline' && error && <p className="mt-2 break-words text-xs text-error">{error}</p>}

          <dl className="mt-4 grid grid-cols-2 gap-2 text-sm">
            <Fact label="Chain" value={runtime ? `${runtime.specName} v${runtime.specVersion}` : '—'} wide />
            <Fact label="Best block" value={best ? formatBlockNumber(best.number) : '—'} />
            <Fact label="Finalized" value={finalized > 0 ? formatBlockNumber(finalized) : '—'} />
            <Fact label="Block time" value={bars.length > 0 ? `~${seconds(blockTime)}` : '—'} />
            <Fact label="Last block" value={best ? `${seconds(now - best.at)} ago` : '—'} />
          </dl>

          <div className="mt-4">
            <p className="text-xs text-tertiary">Last {HISTORY_SLOTS} blocks</p>
            <div className="mt-2 flex h-6 items-end gap-px" data-testid="chain-status-bars">
              {Array.from({ length: HISTORY_SLOTS - bars.length }, (_, i) => (
                <span key={`empty-${i}`} className="h-full flex-1 rounded-[1px] bg-surface-nested" />
              ))}
              {bars.map((bar) => (
                <span
                  key={bar.number}
                  title={`${formatBlockNumber(bar.number)}: ${seconds(bar.delayMs)}, ${BAR[bar.health].word}`}
                  className={cn('h-full flex-1 rounded-[1px]', BAR[bar.health].color)}
                />
              ))}
            </div>
          </div>

          <p className="mt-4 text-xs text-tertiary">Connected through your host.</p>
        </div>
      )}
    </div>
  )
}

function Dot({ className, pulse }: { className: string; pulse: boolean }) {
  return (
    <span className="relative flex h-2 w-2">
      {pulse && <span className={cn('absolute inline-flex h-full w-full animate-ping rounded-full opacity-75 motion-reduce:animate-none', className)} />}
      <span className={cn('relative inline-flex h-2 w-2 rounded-full', className)} />
    </span>
  )
}

function Fact({ label, value, wide }: { label: string; value: string; wide?: boolean }) {
  return (
    <div className={cn('rounded-small bg-surface-nested px-3 py-2', wide && 'col-span-2')}>
      <dt className="text-xs text-tertiary">{label}</dt>
      <dd className="mt-0.5 truncate font-medium tabular-nums text-primary">{value}</dd>
    </div>
  )
}

function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`
}

/** The current time, updated every `intervalMs`. */
function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(timer)
  }, [intervalMs])
  return now
}
