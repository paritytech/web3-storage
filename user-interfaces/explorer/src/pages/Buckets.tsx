// SPDX-License-Identifier: GPL-3.0-only

import { Fragment, useMemo, useState } from 'react'
import { Database } from 'lucide-react'
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
import { useSnapshot } from '@/state/explorer.state'
import { useIsMine } from '@/state/wallet.state'
import { bucketQuotas, type BucketRow } from '@/lib/explorer-client'
import { formatAddress, formatBytes } from '@/utils/format'
import { cn } from '@/utils/cn'

export function Buckets() {
  const snapshot = useSnapshot()
  const isMine = useIsMine()
  const [query, setQuery] = useState('')
  const [expanded, setExpanded] = useState<number | null>(null)

  const quotas = useMemo(
    () => (snapshot ? bucketQuotas(snapshot.agreements) : new Map<number, bigint>()),
    [snapshot]
  )

  const rows = useMemo(() => {
    if (!snapshot) return []
    const q = query.trim().toLowerCase()
    if (!q) return snapshot.buckets
    return snapshot.buckets.filter((b) => {
      const haystack = [
        String(b.id),
        b.visibility ?? '',
        ...b.members.map((m) => `${m.account} ${m.role}`),
        ...b.primaryProviders,
      ]
        .join(' ')
        .toLowerCase()
      return haystack.includes(q)
    })
  }, [snapshot, query])

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold text-gray-100">Buckets</h1>
        <p className="mt-1 text-sm text-gray-400">
          Every bucket on chain, with its members and committed storage quota.
        </p>
      </div>

      <SearchInput
        value={query}
        onChange={setQuery}
        placeholder="Search by bucket id, member, or provider…"
        area="buckets"
      />

      {!snapshot ? (
        <div className="flex justify-center py-16">
          <Spinner size="lg" />
        </div>
      ) : snapshot.failedSections.includes('buckets') ? (
        <SectionUnavailable section="buckets" />
      ) : rows.length === 0 ? (
        <Card className="flex flex-col items-center gap-2 py-16 text-center">
          <Database className="h-8 w-8 text-gray-600" />
          <p className="text-gray-400">
            {query ? `No buckets match "${query}"` : 'No buckets yet'}
          </p>
        </Card>
      ) : (
        <Card>
          <Table data-testid="buckets-table">
            <TableHeader>
              <TableRow>
                <TableHead>Bucket</TableHead>
                <TableHead>Members</TableHead>
                <TableHead>Min providers</TableHead>
                <TableHead>Primary providers</TableHead>
                <TableHead>Committed quota</TableHead>
                <TableHead>Snapshots</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((b) => {
                const mine = b.members.some((m) => isMine(m.account))
                return (
                  <Fragment key={b.id}>
                    <TableRow
                      className={cn(
                        'cursor-pointer',
                        mine && 'border-l-2 border-l-purple-500 bg-purple-500/5'
                      )}
                      data-testid={`buckets-row-${b.id}`}
                      data-mine={mine || undefined}
                      tabIndex={0}
                      role="button"
                      aria-expanded={expanded === b.id}
                      onClick={() => setExpanded(expanded === b.id ? null : b.id)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault()
                          setExpanded(expanded === b.id ? null : b.id)
                        }
                      }}
                    >
                      <TableCell>
                        #{b.id}
                        {mine && (
                          <Badge className="ml-2" variant="default">
                            you
                          </Badge>
                        )}
                        {/* Private is the on-chain default — badge only the exception. */}
                        {b.visibility === 'Public' && (
                          <Badge
                            className="ml-2"
                            variant="secondary"
                            title="Primaries serve reads to anyone (replicas serve everyone regardless of visibility)"
                          >
                            public
                          </Badge>
                        )}
                        {b.frozen && (
                          <Badge className="ml-2" variant="warning">
                            frozen
                          </Badge>
                        )}
                      </TableCell>
                      <TableCell>{b.members.length}</TableCell>
                      <TableCell>{b.minProviders}</TableCell>
                      <TableCell>{b.primaryProviders.length}</TableCell>
                      <TableCell>
                        {snapshot.failedSections.includes('agreements')
                          ? '—'
                          : formatBytes(quotas.get(b.id) ?? 0n)}
                      </TableCell>
                      <TableCell>
                        {b.totalSnapshots.toLocaleString()}
                        {b.hasSnapshot && (
                          <Badge className="ml-2" variant="secondary">
                            checkpointed
                          </Badge>
                        )}
                      </TableCell>
                    </TableRow>
                    {expanded === b.id && <BucketDetails bucket={b} isMine={isMine} />}
                  </Fragment>
                )
              })}
            </TableBody>
          </Table>
        </Card>
      )}
    </div>
  )
}

function BucketDetails({
  bucket: b,
  isMine,
}: {
  bucket: BucketRow
  isMine: (address: string) => boolean
}) {
  return (
    <TableRow className="bg-gray-900/80 hover:bg-gray-900/80">
      <TableCell colSpan={6}>
        <div className="grid gap-6 py-2 md:grid-cols-2">
          <div>
            <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-gray-500">
              Members
            </p>
            <ul className="space-y-1 text-sm">
              {b.members.map((m) => (
                <li key={m.account} className="flex items-center gap-2">
                  <span className="font-mono text-gray-200" title={m.account}>
                    {formatAddress(m.account, 8)}
                  </span>
                  <Badge variant="secondary">{m.role}</Badge>
                  {isMine(m.account) && <Badge variant="default">you</Badge>}
                </li>
              ))}
            </ul>
          </div>
          <div>
            <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-gray-500">
              Primary providers
            </p>
            {b.primaryProviders.length === 0 ? (
              <p className="text-sm text-gray-500">None assigned</p>
            ) : (
              <ul className="space-y-1 text-sm">
                {b.primaryProviders.map((addr) => (
                  <li key={addr} className="flex items-center gap-2">
                    <span className="font-mono text-gray-200" title={addr}>
                      {formatAddress(addr, 8)}
                    </span>
                    {isMine(addr) && <Badge variant="default">you</Badge>}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </TableCell>
    </TableRow>
  )
}
