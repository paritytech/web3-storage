import { useEffect } from 'react'
import { Server, HardDrive, FileText, Shield, Coins, AlertTriangle } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import { Progress } from '@/components/ui/Progress'
import { Spinner } from '@/components/ui/Spinner'
import {
  useProviderInfo,
  useProviderSettings,
  useActiveAgreements,
  usePendingChallenges,
  useEarnings,
  useCapacityUsage,
  useIsProviderLoading,
  loadProviderData,
} from '@/state/provider.state'
import { useSelectedAccount } from '@/state/wallet.state'
import { RequireProvider } from '@/components/RequireProvider'
import { formatBytes, formatTokens, formatDuration } from '@/utils/format'

function StatCard({
  title,
  value,
  description,
  icon: Icon,
  variant = 'default',
}: {
  title: string
  value: string | number
  description?: string
  icon: React.ElementType
  variant?: 'default' | 'success' | 'warning' | 'destructive'
}) {
  const variantClasses = {
    default: 'text-purple-400',
    success: 'text-green-400',
    warning: 'text-yellow-400',
    destructive: 'text-red-400',
  }

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="text-sm font-medium text-gray-400">{title}</CardTitle>
        <Icon className={`h-4 w-4 ${variantClasses[variant]}`} />
      </CardHeader>
      <CardContent>
        <div className="text-2xl font-bold">{value}</div>
        {description && <p className="text-xs text-gray-500 mt-1">{description}</p>}
      </CardContent>
    </Card>
  )
}

export function Overview() {
  const selectedAccount = useSelectedAccount()

  useEffect(() => {
    if (selectedAccount?.address) {
      loadProviderData(selectedAccount.address)
    }
  }, [selectedAccount?.address])

  return (
    <RequireProvider icon={Server} pageName="your provider dashboard">
      <OverviewContent />
    </RequireProvider>
  )
}

function OverviewContent() {
  const providerInfo = useProviderInfo()
  const settings = useProviderSettings()
  const activeAgreements = useActiveAgreements()
  const pendingChallenges = usePendingChallenges()
  const earnings = useEarnings()
  const capacityUsage = useCapacityUsage()
  const isLoading = useIsProviderLoading()

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-16">
        <Spinner size="lg" />
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Provider Overview</h1>
          <p className="text-gray-400">Monitor your storage provider status and performance</p>
        </div>
        <Badge variant={settings?.acceptingPrimary ? 'success' : 'secondary'}>
          {settings?.acceptingPrimary ? 'Accepting Agreements' : 'Not Accepting'}
        </Badge>
      </div>

      {/* Alert for pending challenges */}
      {pendingChallenges.length > 0 && (
        <Card className="border-yellow-500/50 bg-yellow-500/10">
          <CardContent className="flex items-center gap-4 py-4">
            <AlertTriangle className="h-6 w-6 text-yellow-500" />
            <div>
              <p className="font-medium text-yellow-400">
                {pendingChallenges.length} Pending Challenge
                {pendingChallenges.length > 1 ? 's' : ''}
              </p>
              <p className="text-sm text-yellow-500/80">
                Respond promptly to avoid slashing
              </p>
            </div>
            <a
              href="/challenges"
              className="ml-auto px-3 py-1 bg-yellow-500 text-black text-sm rounded-md hover:bg-yellow-400 transition-colors"
            >
              View Challenges
            </a>
          </CardContent>
        </Card>
      )}

      {/* Stats Grid */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <StatCard
          title="Total Stake"
          value={providerInfo ? formatTokens(providerInfo.stake) : '0'}
          icon={Coins}
        />
        <StatCard
          title="Active Agreements"
          value={activeAgreements.length}
          description={`${providerInfo?.bucketCount || 0} total buckets`}
          icon={FileText}
        />
        <StatCard
          title="Pending Challenges"
          value={pendingChallenges.length}
          icon={Shield}
          variant={pendingChallenges.length > 0 ? 'warning' : 'default'}
        />
        <StatCard
          title="Total Earned"
          value={earnings ? formatTokens(earnings.totalEarned) : '0'}
          description={earnings ? `${formatTokens(earnings.pendingPayouts)} pending` : undefined}
          icon={Coins}
          variant="success"
        />
      </div>

      {/* Capacity Usage */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle>Storage Capacity</CardTitle>
              <CardDescription>
                {providerInfo
                  ? providerInfo.capacity > 0
                    ? `${formatBytes(providerInfo.usedCapacity)} used of ${formatBytes(providerInfo.capacity)}`
                    : `${formatBytes(providerInfo.usedCapacity)} committed (no capacity limit set)`
                  : 'No capacity data'}
              </CardDescription>
            </div>
            <HardDrive className="h-5 w-5 text-gray-400" />
          </div>
        </CardHeader>
        <CardContent>
          <Progress value={capacityUsage} />
          <p className="text-sm text-gray-500 mt-2">{capacityUsage}% utilized</p>
        </CardContent>
      </Card>

      {/* Provider Settings Summary */}
      <Card>
        <CardHeader>
          <CardTitle>Current Settings</CardTitle>
          <CardDescription>Your active provider configuration</CardDescription>
        </CardHeader>
        <CardContent>
          {settings && (
            <div className="grid gap-4 md:grid-cols-3">
              <div>
                <p className="text-sm text-gray-400">Price per Byte</p>
                <p className="font-medium">{formatTokens(settings.pricePerByte)}</p>
              </div>
              <div>
                <p className="text-sm text-gray-400">Duration Range</p>
                <p className="font-medium">
                  {formatDuration(settings.minDuration)} - {formatDuration(settings.maxDuration)}
                </p>
              </div>
              <div>
                <p className="text-sm text-gray-400">Max Capacity</p>
                <p className="font-medium">{formatBytes(Number(settings.maxCapacity))}</p>
              </div>
              <div>
                <p className="text-sm text-gray-400">Accepting Primary</p>
                <Badge variant={settings.acceptingPrimary ? 'success' : 'secondary'}>
                  {settings.acceptingPrimary ? 'Yes' : 'No'}
                </Badge>
              </div>
              <div>
                <p className="text-sm text-gray-400">Accepting Replica</p>
                <Badge variant={settings.acceptingReplica ? 'success' : 'secondary'}>
                  {settings.acceptingReplica ? 'Yes' : 'No'}
                </Badge>
              </div>
              <div>
                <p className="text-sm text-gray-400">Accepting Extensions</p>
                <Badge variant={settings.acceptingExtensions ? 'success' : 'secondary'}>
                  {settings.acceptingExtensions ? 'Yes' : 'No'}
                </Badge>
              </div>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
