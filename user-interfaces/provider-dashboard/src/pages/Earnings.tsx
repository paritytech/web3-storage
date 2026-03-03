import { Coins, TrendingUp, Clock, Wallet } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import { Progress } from '@/components/ui/Progress'
import {
  useEarnings,
  useActiveAgreements,
  useIsRegistered,
  useProviderInfo,
} from '@/state/provider.state'
import { useSelectedAccount } from '@/state/wallet.state'
import { useBlockNumber } from '@/state/chain.state'
import { formatTokens, formatBytes, formatBlockNumber } from '@/utils/format'

export function Earnings() {
  const selectedAccount = useSelectedAccount()
  const isRegistered = useIsRegistered()
  const earnings = useEarnings()
  const activeAgreements = useActiveAgreements()
  const providerInfo = useProviderInfo()
  const currentBlock = useBlockNumber()

  if (!selectedAccount) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Coins className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Connect Your Wallet</h2>
        <p className="text-gray-500">Connect a wallet to view your earnings</p>
      </div>
    )
  }

  if (!isRegistered) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Coins className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Not Registered</h2>
        <p className="text-gray-500">Register as a provider to view earnings</p>
      </div>
    )
  }

  // Calculate agreement progress for each active agreement
  const agreementProgress = activeAgreements.map((agreement) => {
    const duration = agreement.endBlock - agreement.startBlock
    const elapsed = Math.max(0, Math.min(currentBlock - agreement.startBlock, duration))
    const progress = (elapsed / duration) * 100
    const earnedSoFar =
      (agreement.pricePerByte * agreement.maxBytes * BigInt(elapsed)) / BigInt(duration)
    const totalValue = agreement.pricePerByte * agreement.maxBytes

    return {
      ...agreement,
      progress,
      earnedSoFar,
      totalValue,
      remaining: totalValue - earnedSoFar,
    }
  })

  const totalPotentialEarnings = agreementProgress.reduce(
    (sum, a) => sum + a.totalValue,
    0n
  )
  const earnedFromActive = agreementProgress.reduce(
    (sum, a) => sum + a.earnedSoFar,
    0n
  )

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold">Earnings</h1>
        <p className="text-gray-400">Track your storage provider earnings and revenue</p>
      </div>

      {/* Overview Stats */}
      <div className="grid gap-4 md:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Earned</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              <TrendingUp className="h-5 w-5 text-green-500" />
              <span className="text-2xl font-bold text-green-400">
                {earnings ? formatTokens(earnings.totalEarned) : '0'}
              </span>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Pending Payouts</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              <Clock className="h-5 w-5 text-yellow-500" />
              <span className="text-2xl font-bold text-yellow-400">
                {earnings ? formatTokens(earnings.pendingPayouts) : '0'}
              </span>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Active Contracts Value</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {formatTokens(totalPotentialEarnings)}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Staked</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              <Wallet className="h-5 w-5 text-purple-500" />
              <span className="text-2xl font-bold">
                {providerInfo ? formatTokens(providerInfo.stake) : '0'}
              </span>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Active Agreement Earnings */}
      <Card>
        <CardHeader>
          <CardTitle>Active Agreement Earnings</CardTitle>
          <CardDescription>
            Earnings progress for your current storage agreements
          </CardDescription>
        </CardHeader>
        <CardContent>
          {agreementProgress.length === 0 ? (
            <div className="text-center py-12">
              <Coins className="h-12 w-12 text-gray-600 mx-auto mb-4" />
              <p className="text-gray-400">No active agreements</p>
              <p className="text-sm text-gray-500 mt-1">
                Start accepting storage agreements to earn
              </p>
            </div>
          ) : (
            <div className="space-y-6">
              {agreementProgress.map((agreement) => (
                <div key={agreement.id} className="space-y-2">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-3">
                      <span className="font-mono text-sm">Bucket #{agreement.bucketId}</span>
                      <Badge variant={agreement.isPrimary ? 'default' : 'secondary'}>
                        {agreement.isPrimary ? 'Primary' : 'Replica'}
                      </Badge>
                      <span className="text-sm text-gray-500">
                        {formatBytes(Number(agreement.maxBytes))}
                      </span>
                    </div>
                    <div className="text-right">
                      <span className="font-medium">{formatTokens(agreement.earnedSoFar)}</span>
                      <span className="text-gray-500 mx-1">/</span>
                      <span className="text-gray-400">{formatTokens(agreement.totalValue)}</span>
                    </div>
                  </div>
                  <Progress value={agreement.progress} />
                  <div className="flex justify-between text-xs text-gray-500">
                    <span>
                      Block {formatBlockNumber(agreement.startBlock)}
                    </span>
                    <span>{agreement.progress.toFixed(1)}% complete</span>
                    <span>
                      Block {formatBlockNumber(agreement.endBlock)}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      {/* Earnings Summary */}
      <Card>
        <CardHeader>
          <CardTitle>Earnings Summary</CardTitle>
          <CardDescription>Overview of your provider economics</CardDescription>
        </CardHeader>
        <CardContent>
          <div className="grid gap-6 md:grid-cols-2">
            <div className="space-y-4">
              <div className="flex justify-between items-center py-2 border-b border-gray-800">
                <span className="text-gray-400">Lifetime Earnings</span>
                <span className="font-medium">
                  {earnings ? formatTokens(earnings.totalEarned) : '0'}
                </span>
              </div>
              <div className="flex justify-between items-center py-2 border-b border-gray-800">
                <span className="text-gray-400">Earned from Active</span>
                <span className="font-medium">{formatTokens(earnedFromActive)}</span>
              </div>
              <div className="flex justify-between items-center py-2 border-b border-gray-800">
                <span className="text-gray-400">Remaining from Active</span>
                <span className="font-medium">
                  {formatTokens(totalPotentialEarnings - earnedFromActive)}
                </span>
              </div>
            </div>
            <div className="space-y-4">
              <div className="flex justify-between items-center py-2 border-b border-gray-800">
                <span className="text-gray-400">Active Agreements</span>
                <span className="font-medium">{activeAgreements.length}</span>
              </div>
              <div className="flex justify-between items-center py-2 border-b border-gray-800">
                <span className="text-gray-400">Last Payout Block</span>
                <span className="font-medium">
                  {earnings?.lastPayoutBlock
                    ? formatBlockNumber(earnings.lastPayoutBlock)
                    : 'N/A'}
                </span>
              </div>
              <div className="flex justify-between items-center py-2 border-b border-gray-800">
                <span className="text-gray-400">Return on Stake</span>
                <span className="font-medium">
                  {providerInfo && providerInfo.stake > 0n
                    ? `${(
                        (Number(earnings?.totalEarned || 0n) / Number(providerInfo.stake)) *
                        100
                      ).toFixed(2)}%`
                    : '0%'}
                </span>
              </div>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
