// SPDX-License-Identifier: GPL-3.0-only

import { useMemo } from 'react'
import { StatTile } from '@/components/StatTile'
import { useAnchorBlock } from '@/state/chain.state'
import { useSnapshot } from '@/state/explorer.state'
import { summarize } from '@/lib/explorer-client'
import { formatBytes, formatTokens } from '@web3-storage/format'

export function Summary() {
  const snapshot = useSnapshot()
  const anchorBlock = useAnchorBlock()

  const stats = useMemo(
    () => (snapshot ? summarize(snapshot, anchorBlock) : null),
    [snapshot, anchorBlock]
  )

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold text-gray-100">Network Summary</h1>
        <p className="mt-1 text-sm text-gray-400">
          What is happening on-chain across the whole network.
        </p>
      </div>

      {snapshot && snapshot.failedSections.length > 0 && (
        <div className="rounded-md border border-yellow-900 bg-yellow-950/50 p-3 text-sm text-yellow-300">
          Some sections could not be loaded from this network: {snapshot.failedSections.join(', ')}
        </div>
      )}

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        <StatTile
          label="Providers"
          value={stats ? stats.providerCount.toLocaleString() : null}
          testId="summary-stat-providers"
        />
        <StatTile
          label="Total stake locked"
          value={stats ? formatTokens(stats.totalStake) : null}
          testId="summary-stat-stake"
        />
        <StatTile
          label="Data under agreement"
          value={stats ? formatBytes(stats.totalData) : null}
          testId="summary-stat-data"
          sub="Committed quota across all providers"
        />
        <StatTile
          label="Active agreements"
          value={stats ? stats.activeAgreements.toLocaleString() : null}
          testId="summary-stat-agreements"
        />
        <StatTile
          label="Buckets"
          value={stats ? stats.bucketCount.toLocaleString() : null}
          testId="summary-stat-buckets"
          sub={snapshot ? `${snapshot.bucketsEverCreated.toLocaleString()} ever created` : undefined}
        />
        <StatTile
          label="Open challenges"
          value={stats ? stats.openChallenges.toLocaleString() : null}
          testId="summary-stat-challenges"
        />
      </div>
    </div>
  )
}
