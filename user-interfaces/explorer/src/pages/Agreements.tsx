// SPDX-License-Identifier: GPL-3.0-only

import { useMemo, useState } from 'react'
import { FileText } from 'lucide-react'
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
import { useAnchorBlock } from '@/state/chain.state'
import { useSnapshot } from '@/state/explorer.state'
import { useIsMine } from '@/state/wallet.state'
import { agreementStatus, type AgreementStatus } from '@/lib/explorer-client'
import { formatAddress, formatBytes, formatTokens } from '@/utils/format'
import { cn } from '@/utils/cn'

const statusVariant: Record<AgreementStatus, 'success' | 'secondary'> = {
  active: 'success',
  expired: 'secondary',
}

export function Agreements() {
  const snapshot = useSnapshot()
  const anchorBlock = useAnchorBlock()
  const isMine = useIsMine()
  const [query, setQuery] = useState('')

  const rows = useMemo(() => {
    if (!snapshot) return []
    const withStatus = snapshot.agreements.map((a) => ({
      ...a,
      status: agreementStatus(a, anchorBlock),
    }))
    const q = query.trim().toLowerCase()
    if (!q) return withStatus
    return withStatus.filter((a) =>
      `${a.provider} ${a.owner} ${a.bucketId} ${a.role} ${a.status}`.toLowerCase().includes(q)
    )
  }, [snapshot, anchorBlock, query])

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold text-gray-100">Agreements</h1>
        <p className="mt-1 text-sm text-gray-400">
          Every storage agreement on chain — who stores what, for whom, until when.
        </p>
      </div>

      <SearchInput
        value={query}
        onChange={setQuery}
        placeholder="Search by provider, owner, bucket, role, or status…"
        area="agreements"
      />

      {!snapshot ? (
        <div className="flex justify-center py-16">
          <Spinner size="lg" />
        </div>
      ) : snapshot.failedSections.includes('agreements') ? (
        <SectionUnavailable section="agreements" />
      ) : rows.length === 0 ? (
        <Card className="flex flex-col items-center gap-2 py-16 text-center">
          <FileText className="h-8 w-8 text-gray-600" />
          <p className="text-gray-400">
            {query ? `No agreements match "${query}"` : 'No agreements yet'}
          </p>
        </Card>
      ) : (
        <Card>
          <Table data-testid="agreements-table">
            <TableHeader>
              <TableRow>
                <TableHead>Bucket</TableHead>
                <TableHead>Provider</TableHead>
                <TableHead>Owner</TableHead>
                <TableHead>Role</TableHead>
                <TableHead>Size</TableHead>
                <TableHead>Price per byte</TableHead>
                <TableHead>Started</TableHead>
                <TableHead>Expires</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((a) => {
                const mine = isMine(a.provider) || isMine(a.owner)
                return (
                  <TableRow
                    key={`${a.bucketId}-${a.provider}`}
                    className={cn(mine && 'border-l-2 border-l-purple-500 bg-purple-500/5')}
                    data-testid={`agreements-row-${a.bucketId}-${a.provider}`}
                    data-mine={mine || undefined}
                  >
                    <TableCell>#{a.bucketId}</TableCell>
                    <TableCell>
                      <AddressCell address={a.provider} mine={isMine(a.provider)} />
                    </TableCell>
                    <TableCell>
                      <AddressCell address={a.owner} mine={isMine(a.owner)} />
                    </TableCell>
                    <TableCell className="text-gray-300">{a.role}</TableCell>
                    <TableCell>{formatBytes(a.maxBytes)}</TableCell>
                    <TableCell>{formatTokens(a.pricePerByte)}</TableCell>
                    <TableCell className="text-gray-400">
                      #{a.startedAt.toLocaleString()}
                    </TableCell>
                    <TableCell className="text-gray-400">
                      #{a.expiresAt.toLocaleString()}
                    </TableCell>
                    <TableCell>
                      <Badge variant={statusVariant[a.status]}>{a.status}</Badge>
                      {a.extensionsBlocked && (
                        <Badge className="ml-1" variant="warning" title="The provider will not extend this agreement; it remains live until it expires">
                          no renewals
                        </Badge>
                      )}
                    </TableCell>
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

export function AddressCell({ address, mine }: { address: string; mine: boolean }) {
  return (
    <span className="whitespace-nowrap">
      <span className="font-mono text-gray-200" title={address}>
        {formatAddress(address)}
      </span>
      {mine && (
        <Badge className="ml-2" variant="default">
          you
        </Badge>
      )}
    </span>
  )
}
