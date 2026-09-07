// SPDX-License-Identifier: GPL-3.0-only

import { useMemo, useState } from 'react'
import { Shield } from 'lucide-react'
import {
  Badge,
  Card,
  Spinner,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui'
import { SearchInput } from '@/components/SearchInput'
import { SectionUnavailable } from '@/components/SectionUnavailable'
import { StatTile } from '@/components/StatTile'
import { AddressCell } from '@/pages/Agreements'
import { useAnchorBlock } from '@/state/chain.state'
import { useSnapshot } from '@/state/explorer.state'
import { useIsMine } from '@/state/wallet.state'
import { formatDuration, formatTokens } from '@/utils/format'
import { cn } from '@/utils/cn'

export function Challenges() {
  const snapshot = useSnapshot()
  const anchorBlock = useAnchorBlock()
  const isMine = useIsMine()
  const [query, setQuery] = useState('')

  const rows = useMemo(() => {
    if (!snapshot) return []
    const q = query.trim().toLowerCase()
    if (!q) return snapshot.openChallenges
    return snapshot.openChallenges.filter((c) =>
      `${c.provider} ${c.challenger} ${c.bucketId} ${c.deadline} ${
        c.authorized ? 'authorized' : 'public'
      }`
        .toLowerCase()
        .includes(q)
    )
  }, [snapshot, query])

  // A failed aggregates query must not render as a confident "0 challenges".
  const agg =
    snapshot && !snapshot.failedSections.includes('challenge stats')
      ? snapshot.challengeAggregates
      : null
  const aggFailed = snapshot?.failedSections.includes('challenge stats') ?? false

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold text-gray-100">Challenges</h1>
        <p className="mt-1 text-sm text-gray-400">
          Open challenges awaiting a response. Resolved challenges are removed from chain
          storage — only the outcome totals below remain.
        </p>
      </div>

      <div className="grid gap-4 md:grid-cols-3">
        <StatTile
          label="Challenges ever raised"
          value={aggFailed ? '—' : agg ? agg.totalIssued.toLocaleString() : null}
          testId="challenges-stat-issued"
        />
        <StatTile
          label="Upheld (provider slashed)"
          value={aggFailed ? '—' : agg ? agg.upheld.toLocaleString() : null}
          testId="challenges-stat-upheld"
        />
        <StatTile
          label="Dismissed (provider defended)"
          value={aggFailed ? '—' : agg ? agg.dismissed.toLocaleString() : null}
          testId="challenges-stat-dismissed"
        />
      </div>

      <SearchInput
        value={query}
        onChange={setQuery}
        placeholder="Search by provider, challenger, or bucket…"
        area="challenges"
      />

      {!snapshot ? (
        <div className="flex justify-center py-16">
          <Spinner size="lg" />
        </div>
      ) : snapshot.failedSections.includes('challenges') ? (
        <SectionUnavailable section="challenges" />
      ) : rows.length === 0 ? (
        <Card className="flex flex-col items-center gap-2 py-16 text-center">
          <Shield className="h-8 w-8 text-gray-600" />
          <p className="text-gray-400">
            {query ? `No open challenges match "${query}"` : 'No open challenges'}
          </p>
        </Card>
      ) : (
        <Card>
          <Table data-testid="challenges-table">
            <TableHeader>
              <TableRow>
                <TableHead>Deadline</TableHead>
                <TableHead>Bucket</TableHead>
                <TableHead>Provider</TableHead>
                <TableHead>Challenger</TableHead>
                <TableHead>Target</TableHead>
                <TableHead>Deposit</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((c) => {
                const mine = isMine(c.provider) || isMine(c.challenger)
                const overdue = anchorBlock > 0 && anchorBlock > c.deadline
                return (
                  <TableRow
                    key={`${c.deadline}-${c.index}`}
                    className={cn(mine && 'border-l-2 border-l-purple-500 bg-purple-500/5')}
                    data-testid={`challenges-row-${c.deadline}-${c.index}`}
                    data-mine={mine || undefined}
                  >
                    <TableCell>
                      #{c.deadline.toLocaleString()}
                      {overdue ? (
                        <Badge className="ml-2" variant="destructive">
                          overdue
                        </Badge>
                      ) : (
                        anchorBlock > 0 && (
                          <span className="ml-2 text-xs text-gray-500">
                            due in {formatDuration(c.deadline - anchorBlock)}
                          </span>
                        )
                      )}
                    </TableCell>
                    <TableCell>#{c.bucketId}</TableCell>
                    <TableCell>
                      <AddressCell address={c.provider} mine={isMine(c.provider)} />
                    </TableCell>
                    <TableCell>
                      <AddressCell address={c.challenger} mine={isMine(c.challenger)} />
                      {c.authorized ? (
                        <Badge
                          className="ml-1"
                          variant="success"
                          title="Bucket member or agreement owner at challenge creation"
                        >
                          authorized
                        </Badge>
                      ) : (
                        <Badge
                          className="ml-1"
                          variant="outline"
                          title="General-public challenger"
                        >
                          public
                        </Badge>
                      )}
                    </TableCell>
                    <TableCell className="text-gray-400">
                      leaf {c.leafIndex}, chunk {c.chunkIndex}
                    </TableCell>
                    <TableCell>{formatTokens(c.deposit)}</TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </Card>
      )}
    </div>
  )
}
