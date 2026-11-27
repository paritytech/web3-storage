// SPDX-License-Identifier: GPL-3.0-only
//
// How much of the plan the library uses, as a bar and a line of text.

import type { RemoteStoragePlan } from '@parity/truapi'
import { formatBytesBinary } from '@web3-storage/format'

export function StorageUsage({ plan }: { plan: RemoteStoragePlan }) {
  const share = plan.bytes > 0n ? Math.min(1, Number(plan.usedBytes) / Number(plan.bytes)) : 0
  return (
    <div data-testid="plan-summary">
      <div className="h-1.5 overflow-hidden rounded-full bg-surface-nested">
        <div
          className={share >= 0.9 ? 'h-full rounded-full bg-warning' : 'h-full rounded-full bg-action-primary'}
          style={{ width: `${share * 100}%` }}
        />
      </div>
      <p className="mt-2 text-xs text-secondary">
        {formatBytesBinary(plan.usedBytes)} of {formatBytesBinary(plan.bytes)} used
      </p>
      <p className="mt-0.5 text-xs text-tertiary">
        Paid until {new Date(Number(plan.fundedUntil) * 1000).toLocaleDateString()}
      </p>
    </div>
  )
}
