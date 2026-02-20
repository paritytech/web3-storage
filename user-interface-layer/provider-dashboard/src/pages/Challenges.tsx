import { useState } from 'react'
import { Shield, AlertTriangle, Clock, CheckCircle, XCircle } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import { Button } from '@/components/ui/Button'
import { Spinner } from '@/components/ui/Spinner'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/Table'
import {
  useChallenges,
  usePendingChallenges,
  useIsRegistered,
  respondToChallenge,
} from '@/state/provider.state'
import { useSelectedAccount } from '@/state/wallet.state'
import { formatAddress, formatBlockNumber, formatRelativeTime, formatDuration } from '@/utils/format'

export function Challenges() {
  const selectedAccount = useSelectedAccount()
  const isRegistered = useIsRegistered()
  const challenges = useChallenges()
  const pendingChallenges = usePendingChallenges()
  const [respondingTo, setRespondingTo] = useState<number | null>(null)

  const handleRespond = async (challengeId: number) => {
    setRespondingTo(challengeId)
    try {
      // In a real implementation, this would fetch the proof from the provider node
      const mockProof = new Uint8Array(64)
      await respondToChallenge(challengeId, mockProof)
    } catch (error) {
      console.error('Failed to respond to challenge:', error)
    } finally {
      setRespondingTo(null)
    }
  }

  if (!selectedAccount) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Shield className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Connect Your Wallet</h2>
        <p className="text-gray-500">Connect a wallet to view challenges</p>
      </div>
    )
  }

  if (!isRegistered) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Shield className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Not Registered</h2>
        <p className="text-gray-500">Register as a provider to view challenges</p>
      </div>
    )
  }

  const getStatusIcon = (status: string) => {
    switch (status) {
      case 'pending':
        return <Clock className="h-4 w-4 text-yellow-500" />
      case 'responded':
        return <CheckCircle className="h-4 w-4 text-green-500" />
      case 'slashed':
        return <XCircle className="h-4 w-4 text-red-500" />
      case 'expired':
        return <Clock className="h-4 w-4 text-gray-500" />
      default:
        return null
    }
  }

  const getStatusVariant = (status: string): 'warning' | 'success' | 'destructive' | 'secondary' => {
    switch (status) {
      case 'pending':
        return 'warning'
      case 'responded':
        return 'success'
      case 'slashed':
        return 'destructive'
      default:
        return 'secondary'
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold">Challenges</h1>
        <p className="text-gray-400">
          Respond to data availability challenges to avoid slashing
        </p>
      </div>

      {/* Alert for pending challenges */}
      {pendingChallenges.length > 0 && (
        <Card className="border-yellow-500/50 bg-yellow-500/10">
          <CardContent className="py-4">
            <div className="flex items-start gap-3">
              <AlertTriangle className="h-5 w-5 text-yellow-500 mt-0.5" />
              <div>
                <p className="font-medium text-yellow-400">
                  {pendingChallenges.length} Pending Challenge
                  {pendingChallenges.length > 1 ? 's' : ''} Require Attention
                </p>
                <p className="text-sm text-yellow-500/80 mt-1">
                  Respond before the deadline to avoid having your stake slashed. The provider
                  node should automatically generate proofs for valid data.
                </p>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Stats */}
      <div className="grid gap-4 md:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Pending</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-yellow-400">
              {challenges.filter((c) => c.status === 'pending').length}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Responded</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-green-400">
              {challenges.filter((c) => c.status === 'responded').length}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Slashed</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-red-400">
              {challenges.filter((c) => c.status === 'slashed').length}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Expired</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {challenges.filter((c) => c.status === 'expired').length}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Challenges Table */}
      <Card>
        <CardHeader>
          <CardTitle>All Challenges</CardTitle>
          <CardDescription>History of challenges against your stored data</CardDescription>
        </CardHeader>
        <CardContent>
          {challenges.length === 0 ? (
            <div className="text-center py-12">
              <Shield className="h-12 w-12 text-gray-600 mx-auto mb-4" />
              <p className="text-gray-400">No challenges yet</p>
              <p className="text-sm text-gray-500 mt-1">
                Challenges appear when someone questions your data availability
              </p>
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>ID</TableHead>
                  <TableHead>Bucket</TableHead>
                  <TableHead>Challenger</TableHead>
                  <TableHead>Leaf Index</TableHead>
                  <TableHead>Deadline</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead>Action</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {challenges
                  .sort((a, b) => {
                    // Pending first, then by creation time
                    if (a.status === 'pending' && b.status !== 'pending') return -1
                    if (a.status !== 'pending' && b.status === 'pending') return 1
                    return b.createdAt - a.createdAt
                  })
                  .map((challenge) => (
                    <TableRow
                      key={challenge.id}
                      className={
                        challenge.status === 'pending' ? 'bg-yellow-500/5' : undefined
                      }
                    >
                      <TableCell className="font-mono">#{challenge.id}</TableCell>
                      <TableCell className="font-mono">#{challenge.bucketId}</TableCell>
                      <TableCell>
                        <span className="font-mono">{formatAddress(challenge.challenger)}</span>
                      </TableCell>
                      <TableCell>{challenge.leafIndex.toLocaleString()}</TableCell>
                      <TableCell>
                        {challenge.status === 'pending' ? (
                          <span className="text-yellow-400">
                            {formatBlockNumber(challenge.deadline)}
                          </span>
                        ) : (
                          <span className="text-gray-500">
                            {formatBlockNumber(challenge.deadline)}
                          </span>
                        )}
                      </TableCell>
                      <TableCell>
                        <div className="flex items-center gap-2">
                          {getStatusIcon(challenge.status)}
                          <Badge variant={getStatusVariant(challenge.status)}>
                            {challenge.status.charAt(0).toUpperCase() + challenge.status.slice(1)}
                          </Badge>
                        </div>
                      </TableCell>
                      <TableCell>
                        {challenge.status === 'pending' && (
                          <Button
                            size="sm"
                            onClick={() => handleRespond(challenge.id)}
                            disabled={respondingTo === challenge.id}
                          >
                            {respondingTo === challenge.id ? (
                              <>
                                <Spinner size="sm" className="mr-2" />
                                Responding...
                              </>
                            ) : (
                              'Respond'
                            )}
                          </Button>
                        )}
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
