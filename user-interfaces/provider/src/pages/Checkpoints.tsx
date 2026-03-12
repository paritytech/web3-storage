import { CheckCircle, Clock, Hash } from 'lucide-react'
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
import { useCheckpoints } from '@/state/provider.state'
import { RequireProvider } from '@/components/RequireProvider'
import { formatBlockNumber, formatRelativeTime, formatAddress } from '@/utils/format'

export function Checkpoints() {
  return (
    <RequireProvider icon={CheckCircle} pageName="checkpoints">
      <CheckpointsContent />
    </RequireProvider>
  )
}

function CheckpointsContent() {
  const checkpoints = useCheckpoints()

  // Group checkpoints by bucket
  const checkpointsByBucket = checkpoints.reduce((acc, cp) => {
    if (!acc[cp.bucketId]) {
      acc[cp.bucketId] = []
    }
    acc[cp.bucketId].push(cp)
    return acc
  }, {} as Record<number, typeof checkpoints>)

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold">Checkpoints</h1>
        <p className="text-gray-400">
          View and manage submitted checkpoints for your storage buckets
        </p>
      </div>

      {/* Stats */}
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Checkpoints</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{checkpoints.length}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Buckets with Checkpoints</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{Object.keys(checkpointsByBucket).length}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Latest Checkpoint</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {checkpoints.length > 0
                ? formatRelativeTime(Math.max(...checkpoints.map((c) => c.submittedAt)))
                : 'None'}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Info Card */}
      <Card className="bg-purple-500/10 border-purple-500/30">
        <CardContent className="py-4">
          <div className="flex items-start gap-3">
            <Hash className="h-5 w-5 text-purple-400 mt-0.5" />
            <div>
              <p className="font-medium text-purple-300">About Checkpoints</p>
              <p className="text-sm text-purple-400/80 mt-1">
                Checkpoints commit the MMR root of stored data on-chain. This creates a verifiable
                commitment that you hold the data and enables the challenge mechanism if disputes
                arise. Submit checkpoints regularly to minimize potential data loss liability.
              </p>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Checkpoints Table */}
      <Card>
        <CardHeader>
          <CardTitle>Recent Checkpoints</CardTitle>
          <CardDescription>History of submitted checkpoints</CardDescription>
        </CardHeader>
        <CardContent>
          {checkpoints.length === 0 ? (
            <div className="text-center py-12">
              <Clock className="h-12 w-12 text-gray-600 mx-auto mb-4" />
              <p className="text-gray-400">No checkpoints submitted yet</p>
              <p className="text-sm text-gray-500 mt-1">
                Checkpoints will appear here once data is stored and committed
              </p>
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Bucket</TableHead>
                  <TableHead>Block</TableHead>
                  <TableHead>MMR Root</TableHead>
                  <TableHead>Leaves</TableHead>
                  <TableHead>Providers</TableHead>
                  <TableHead>Submitted</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {checkpoints
                  .sort((a, b) => b.submittedAt - a.submittedAt)
                  .map((checkpoint, index) => (
                    <TableRow key={`${checkpoint.bucketId}-${checkpoint.blockNumber}-${index}`}>
                      <TableCell className="font-mono">#{checkpoint.bucketId}</TableCell>
                      <TableCell>{formatBlockNumber(checkpoint.blockNumber)}</TableCell>
                      <TableCell>
                        <code className="text-xs bg-gray-800 px-2 py-1 rounded">
                          {checkpoint.mmrRoot.slice(0, 10)}...{checkpoint.mmrRoot.slice(-8)}
                        </code>
                      </TableCell>
                      <TableCell>{checkpoint.leafCount.toLocaleString()}</TableCell>
                      <TableCell>
                        <div className="flex items-center gap-1">
                          {checkpoint.providers.slice(0, 2).map((p, i) => (
                            <Badge key={i} variant="secondary" className="text-xs">
                              {formatAddress(p, 3)}
                            </Badge>
                          ))}
                          {checkpoint.providers.length > 2 && (
                            <Badge variant="outline" className="text-xs">
                              +{checkpoint.providers.length - 2}
                            </Badge>
                          )}
                        </div>
                      </TableCell>
                      <TableCell className="text-gray-400">
                        {formatRelativeTime(checkpoint.submittedAt)}
                      </TableCell>
                    </TableRow>
                  ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
