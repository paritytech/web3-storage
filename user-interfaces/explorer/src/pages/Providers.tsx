// SPDX-License-Identifier: GPL-3.0-only

import { Fragment, useMemo, useState } from 'react'
import { Server } from 'lucide-react'
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
import { type ProviderRow } from '@/lib/explorer-client'
import {
  formatAddress,
  formatBytes,
  formatDuration,
  formatTokens,
} from '@/utils/format'
import { cn } from '@/utils/cn'

export function Providers() {
  const snapshot = useSnapshot()
  const isMine = useIsMine()
  const [query, setQuery] = useState('')
  const [expanded, setExpanded] = useState<string | null>(null)

  const rows = useMemo(() => {
    if (!snapshot) return []
    const q = query.trim().toLowerCase()
    if (!q) return snapshot.providers
    // Matched in the chain's SS58 encoding; a pasted address under a foreign
    // prefix won't match (byte-level normalization is future work).
    return snapshot.providers.filter((p) =>
      `${p.address} ${p.multiaddr}`.toLowerCase().includes(q)
    )
  }, [snapshot, query])

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold text-gray-100">Providers</h1>
        <p className="mt-1 text-sm text-gray-400">
          Every registered storage provider, with stake, capacity, and settings.
        </p>
      </div>

      <SearchInput
        value={query}
        onChange={setQuery}
        placeholder="Search by address or multiaddr…"
        area="providers"
      />

      {!snapshot ? (
        <div className="flex justify-center py-16">
          <Spinner size="lg" />
        </div>
      ) : snapshot.failedSections.includes('providers') ? (
        <SectionUnavailable section="providers" />
      ) : rows.length === 0 ? (
        <EmptyState query={query} />
      ) : (
        <Card>
          <Table data-testid="providers-table">
            <TableHeader>
              <TableRow>
                <TableHead>Provider</TableHead>
                <TableHead>Multiaddr</TableHead>
                <TableHead>Stake</TableHead>
                <TableHead>Committed / Capacity</TableHead>
                <TableHead>Price per byte</TableHead>
                <TableHead>Accepting</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((p) => {
                const mine = isMine(p.address)
                return (
                  <Fragment key={p.address}>
                    <TableRow
                      className={cn(
                        'cursor-pointer',
                        mine && 'border-l-2 border-l-purple-500 bg-purple-500/5'
                      )}
                      data-testid={`providers-row-${p.address}`}
                      data-mine={mine || undefined}
                      tabIndex={0}
                      role="button"
                      aria-expanded={expanded === p.address}
                      onClick={() => setExpanded(expanded === p.address ? null : p.address)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault()
                          setExpanded(expanded === p.address ? null : p.address)
                        }
                      }}
                    >
                      <TableCell>
                        <span className="font-mono text-gray-200" title={p.address}>
                          {formatAddress(p.address)}
                        </span>
                        {mine && (
                          <Badge className="ml-2" variant="default">
                            you
                          </Badge>
                        )}
                        {p.deregisterAt !== undefined && (
                          <Badge className="ml-2" variant="warning">
                            deregistering
                          </Badge>
                        )}
                      </TableCell>
                      <TableCell className="max-w-48 truncate font-mono text-xs text-gray-400">
                        {p.multiaddr}
                      </TableCell>
                      <TableCell>{formatTokens(p.stake)}</TableCell>
                      <TableCell>
                        {formatBytes(p.committedBytes)} /{' '}
                        {p.settings.maxCapacity === 0n
                          ? 'unlimited'
                          : formatBytes(p.settings.maxCapacity)}
                      </TableCell>
                      <TableCell>{formatTokens(p.settings.pricePerByte)}</TableCell>
                      <TableCell>
                        <div className="flex gap-1">
                          {p.settings.acceptingPrimary && <Badge variant="success">primary</Badge>}
                          {p.settings.replicaSyncPrice !== undefined && (
                            <Badge variant="success">replica</Badge>
                          )}
                          {p.settings.acceptingExtensions && (
                            <Badge variant="secondary">extensions</Badge>
                          )}
                        </div>
                      </TableCell>
                    </TableRow>
                    {expanded === p.address && <ProviderDetails provider={p} />}
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

/**
 * Full detail: the exact 7-field on-chain ProviderSettings struct plus the
 * provider's stats — nothing invented, nothing omitted (issue #122).
 */
function ProviderDetails({ provider: p }: { provider: ProviderRow }) {
  const s = p.settings
  return (
    <TableRow className="bg-gray-900/80 hover:bg-gray-900/80">
      <TableCell colSpan={6}>
        <div className="grid gap-6 py-2 md:grid-cols-2">
          <div>
            <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-gray-500">
              Settings
            </p>
            <dl className="grid grid-cols-2 gap-x-6 gap-y-1 text-sm">
              <dt className="text-gray-400">Min duration</dt>
              <dd className="text-gray-200">{formatDuration(s.minDuration)}</dd>
              <dt className="text-gray-400">Max duration</dt>
              <dd className="text-gray-200">{formatDuration(s.maxDuration)}</dd>
              <dt className="text-gray-400">Price per byte</dt>
              <dd className="text-gray-200">{formatTokens(s.pricePerByte)}</dd>
              <dt className="text-gray-400">Accepting primary</dt>
              <dd className="text-gray-200">{s.acceptingPrimary ? 'Yes' : 'No'}</dd>
              <dt className="text-gray-400">Replica sync price</dt>
              <dd className="text-gray-200">
                {s.replicaSyncPrice === undefined
                  ? 'Not accepting replicas'
                  : formatTokens(s.replicaSyncPrice)}
              </dd>
              <dt className="text-gray-400">Accepting extensions</dt>
              <dd className="text-gray-200">{s.acceptingExtensions ? 'Yes' : 'No'}</dd>
              <dt className="text-gray-400">Max capacity</dt>
              <dd className="text-gray-200">
                {s.maxCapacity === 0n ? 'Unlimited' : formatBytes(s.maxCapacity)}
              </dd>
            </dl>
          </div>
          <div>
            <p className="mb-2 text-xs font-semibold uppercase tracking-wide text-gray-500">
              Stats
            </p>
            <dl className="grid grid-cols-2 gap-x-6 gap-y-1 text-sm">
              <dt className="text-gray-400">Registered at</dt>
              <dd className="text-gray-200">#{p.stats.registeredAt.toLocaleString()}</dd>
              <dt className="text-gray-400">Agreements (total)</dt>
              <dd className="text-gray-200">{p.stats.agreementsTotal.toLocaleString()}</dd>
              <dt className="text-gray-400">Extended / not / burned</dt>
              <dd className="text-gray-200">
                {p.stats.agreementsExtended} / {p.stats.agreementsNotExtended} /{' '}
                {p.stats.agreementsBurned}
              </dd>
              <dt className="text-gray-400">Lifetime bytes committed</dt>
              <dd className="text-gray-200">{formatBytes(p.stats.totalBytesCommitted)}</dd>
              <dt className="text-gray-400">Challenges received</dt>
              <dd className="text-gray-200">{p.stats.challengesReceived.toLocaleString()}</dd>
              <dt className="text-gray-400">Challenges failed</dt>
              <dd className="text-gray-200">{p.stats.challengesFailed.toLocaleString()}</dd>
            </dl>
          </div>
        </div>
      </TableCell>
    </TableRow>
  )
}

function EmptyState({ query }: { query: string }) {
  return (
    <Card className="flex flex-col items-center gap-2 py-16 text-center">
      <Server className="h-8 w-8 text-gray-600" />
      <p className="text-gray-400">
        {query ? `No providers match "${query}"` : 'No providers registered yet'}
      </p>
    </Card>
  )
}
