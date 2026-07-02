// SPDX-License-Identifier: GPL-3.0-only

import { useState, Fragment } from 'react'
import { Database, ChevronDown, ChevronUp, Users, Lock, AlertTriangle, CheckCircle } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/Table'
import { RequireProvider } from '@/components/RequireProvider'
import { useBucketDetails, type BucketDetail } from '@/state/provider.state'
import { formatAddress, formatBytes, formatTokens, formatBlockNumber, formatDuration, formatHash } from '@/utils/format'

export function Buckets() {
  return (
    <RequireProvider icon={Database} pageName="buckets">
      <BucketsContent />
    </RequireProvider>
  )
}

function BucketsContent() {
  const buckets = useBucketDetails()
  const [expandedBucket, setExpandedBucket] = useState<number | null>(null)

  const primaryCount = buckets.filter((b) => b.agreement.isPrimary).length
  const overdueCount = buckets.filter((b) => b.isCheckpointOverdue).length

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold">Buckets</h1>
        <p className="text-gray-400">Layer 0 storage buckets you are serving</p>
      </div>

      {/* Stats */}
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Buckets</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{buckets.length}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Primary Role</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-purple-400">{primaryCount}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Overdue Checkpoints</CardTitle>
          </CardHeader>
          <CardContent>
            <div className={`text-2xl font-bold ${overdueCount > 0 ? 'text-yellow-400' : 'text-green-400'}`}>
              {overdueCount}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Bucket list */}
      <Card>
        <CardHeader>
          <CardTitle>Your Buckets</CardTitle>
          <CardDescription>Detailed view of each L0 bucket you serve</CardDescription>
        </CardHeader>
        <CardContent>
          {buckets.length === 0 ? (
            <div className="text-center py-12">
              <Database className="h-12 w-12 text-gray-600 mx-auto mb-4" />
              <p className="text-gray-400">No buckets</p>
              <p className="text-sm text-gray-500 mt-1">
                Buckets appear when you accept storage agreements
              </p>
            </div>
          ) : (
            <Table data-testid="buckets-table">
              <TableHeader>
                <TableRow>
                  <TableHead>Bucket</TableHead>
                  <TableHead>Admin</TableHead>
                  <TableHead>Role</TableHead>
                  <TableHead>Capacity</TableHead>
                  <TableHead>Expires</TableHead>
                  <TableHead>Checkpoint</TableHead>
                  <TableHead>Providers</TableHead>
                  <TableHead></TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {buckets.map((bucket) => {
                  const isExpanded = expandedBucket === bucket.bucketId
                  const admin = bucket.members.find((m) => m.role === 'Admin')

                  return (
                    <Fragment key={bucket.bucketId}>
                      <TableRow
                        data-testid={`buckets-row-${bucket.bucketId}`}
                        className="cursor-pointer hover:bg-gray-800/50"
                        onClick={() => setExpandedBucket(isExpanded ? null : bucket.bucketId)}
                      >
                        <TableCell>
                          <div className="flex items-center gap-2">
                            <span className="font-mono font-medium">#{bucket.bucketId}</span>
                            {bucket.frozenStartSeq != null && (
                              <Lock className="h-3 w-3 text-blue-400" />
                            )}
                          </div>
                        </TableCell>
                        <TableCell>
                          <span className="font-mono text-sm">{admin ? formatAddress(admin.account) : '—'}</span>
                        </TableCell>
                        <TableCell>
                          <Badge variant={bucket.agreement.isPrimary ? 'default' : 'secondary'}>
                            {bucket.agreement.isPrimary ? 'Primary' : 'Replica'}
                          </Badge>
                        </TableCell>
                        <TableCell>{formatBytes(Number(bucket.agreement.maxBytes))}</TableCell>
                        <TableCell>
                          <span className={bucket.agreement.status === 'active' ? 'text-green-400' : 'text-gray-500'}>
                            {formatBlockNumber(bucket.agreement.expiresAt)}
                          </span>
                        </TableCell>
                        <TableCell>
                          {bucket.snapshot ? (
                            <div className="flex items-center gap-1">
                              {bucket.isCheckpointOverdue ? (
                                <AlertTriangle className="h-3 w-3 text-yellow-400" />
                              ) : (
                                <CheckCircle className="h-3 w-3 text-green-400" />
                              )}
                              <span className="text-sm">{formatBlockNumber(bucket.snapshot.checkpointBlock)}</span>
                            </div>
                          ) : (
                            <span className="text-gray-500">None</span>
                          )}
                        </TableCell>
                        <TableCell>{bucket.primaryProviders.length}</TableCell>
                        <TableCell>
                          {isExpanded ? (
                            <ChevronUp className="h-4 w-4 text-gray-400" />
                          ) : (
                            <ChevronDown className="h-4 w-4 text-gray-400" />
                          )}
                        </TableCell>
                      </TableRow>

                      {isExpanded && (
                        <TableRow key={`${bucket.bucketId}-detail`}>
                          <TableCell colSpan={8} className="bg-gray-900/50 p-0">
                            <BucketExpandedDetail bucket={bucket} />
                          </TableCell>
                        </TableRow>
                      )}
                    </Fragment>
                  )
                })}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

function BucketExpandedDetail({ bucket }: { bucket: BucketDetail }) {
  return (
    <div className="grid gap-6 md:grid-cols-2 p-6">
      {/* Left: Members + Config */}
      <div className="space-y-4">
        <div>
          <h4 className="text-sm font-medium text-gray-400 mb-2 flex items-center gap-2">
            <Users className="h-4 w-4" />
            Members ({bucket.members.length})
          </h4>
          <div className="space-y-1">
            {bucket.members.map((m) => (
              <div key={m.account} className="flex items-center justify-between text-sm bg-gray-800/50 rounded p-2">
                <span className="font-mono">{formatAddress(m.account)}</span>
                <Badge variant={m.role === 'Admin' ? 'destructive' : m.role === 'Writer' ? 'warning' : 'secondary'} className="text-xs">
                  {m.role}
                </Badge>
              </div>
            ))}
          </div>
        </div>

        <div>
          <h4 className="text-sm font-medium text-gray-400 mb-2">Agreement</h4>
          <div className="grid grid-cols-2 gap-2 text-sm">
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Capacity</span>
              <p className="font-medium">{formatBytes(Number(bucket.agreement.maxBytes))}</p>
            </div>
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Started</span>
              <p className="font-medium">{formatBlockNumber(bucket.agreement.startedAt)}</p>
            </div>
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Expires</span>
              <p className="font-medium">{formatBlockNumber(bucket.agreement.expiresAt)}</p>
            </div>
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Payment Locked</span>
              <p className="font-medium">{formatTokens(bucket.agreement.paymentLocked)}</p>
            </div>
          </div>
        </div>

        {bucket.checkpointConfig && (
          <div>
            <h4 className="text-sm font-medium text-gray-400 mb-2">Checkpoint Config</h4>
            <div className="grid grid-cols-3 gap-2 text-sm">
              <div className="bg-gray-800/50 rounded p-2">
                <span className="text-gray-500">Interval</span>
                <p className="font-medium">{formatDuration(bucket.checkpointConfig.interval)}</p>
              </div>
              <div className="bg-gray-800/50 rounded p-2">
                <span className="text-gray-500">Grace Period</span>
                <p className="font-medium">{formatDuration(bucket.checkpointConfig.gracePeriod)}</p>
              </div>
              <div className="bg-gray-800/50 rounded p-2">
                <span className="text-gray-500">Enabled</span>
                <p className="font-medium">{bucket.checkpointConfig.enabled ? 'Yes' : 'No'}</p>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Right: Snapshot + Rewards */}
      <div className="space-y-4">
        <div>
          <h4 className="text-sm font-medium text-gray-400 mb-2">Latest Snapshot</h4>
          {bucket.snapshot ? (
            <div className="space-y-2 text-sm">
              <div className="bg-gray-800/50 rounded p-2">
                <span className="text-gray-500">MMR Root</span>
                <p className="font-mono text-xs break-all">{formatHash(bucket.snapshot.mmrRoot, 10, 10)}</p>
              </div>
              <div className="grid grid-cols-3 gap-2">
                <div className="bg-gray-800/50 rounded p-2">
                  <span className="text-gray-500">Leaves</span>
                  <p className="font-medium">{bucket.snapshot.leafCount}</p>
                </div>
                <div className="bg-gray-800/50 rounded p-2">
                  <span className="text-gray-500">Start Seq</span>
                  <p className="font-medium">{bucket.snapshot.startSeq}</p>
                </div>
                <div className="bg-gray-800/50 rounded p-2">
                  <span className="text-gray-500">Block</span>
                  <p className="font-medium">{formatBlockNumber(bucket.snapshot.checkpointBlock)}</p>
                </div>
              </div>
            </div>
          ) : (
            <p className="text-sm text-gray-500 bg-gray-800/50 rounded p-2">No checkpoint submitted yet</p>
          )}
        </div>

        <div>
          <h4 className="text-sm font-medium text-gray-400 mb-2">Checkpoint Rewards</h4>
          <div className="grid grid-cols-2 gap-2 text-sm">
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Pool Balance</span>
              <p className="font-medium">{formatTokens(bucket.checkpointPoolBalance)}</p>
            </div>
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Your Pending Reward</span>
              <p className="font-medium text-green-400">{formatTokens(bucket.checkpointReward)}</p>
            </div>
          </div>
        </div>

        <div>
          <h4 className="text-sm font-medium text-gray-400 mb-2">Bucket Info</h4>
          <div className="grid grid-cols-3 gap-2 text-sm">
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Min Providers</span>
              <p className="font-medium">{bucket.minProviders}</p>
            </div>
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Total Snapshots</span>
              <p className="font-medium">{bucket.totalSnapshots}</p>
            </div>
            <div className="bg-gray-800/50 rounded p-2">
              <span className="text-gray-500">Frozen</span>
              <p className="font-medium">{bucket.frozenStartSeq != null ? `From seq ${bucket.frozenStartSeq}` : 'No'}</p>
            </div>
          </div>
        </div>

        {bucket.historicalRoots.length > 0 && (
          <div>
            <h4 className="text-sm font-medium text-gray-400 mb-2">Historical Roots</h4>
            <div className="space-y-1 text-xs font-mono">
              {bucket.historicalRoots.map((r, i) => (
                <div key={i} className="bg-gray-800/50 rounded p-2 flex justify-between">
                  <span className="text-gray-500">pos {r.position}</span>
                  <span>{formatHash(r.root)}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
