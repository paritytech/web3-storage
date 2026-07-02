// SPDX-License-Identifier: Apache-2.0

import { useEffect, useState, useCallback } from 'react'
import { Server, HardDrive, FileText, Shield, Coins, AlertTriangle, Network } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import { Button } from '@/components/ui/Button'
import { Progress } from '@/components/ui/Progress'
import { Spinner } from '@/components/ui/Spinner'
import { NetworkStatusPanel } from '@web3-storage/network-picker'
import {
  useProviderInfo,
  useProviderSettings,
  useActiveAgreements,
  usePendingChallenges,
  useEarnings,
  useCapacityUsage,
  useIsProviderLoading,
  loadProviderData,
  deregisterProvider,
  completeDeregister,
} from '@/state/provider.state'
import type { TxStatus } from '@/state/provider.state'
import { useSelectedAccount } from '@/state/wallet.state'
import { useSelectedNetwork, useSelectedNetworkId } from '@/state/network.state'
import { useChainInfo, useConnectionStatus, useBlockNumber } from '@/state/chain.state'
import { RequireProvider } from '@/components/RequireProvider'
import { formatBytes, formatTokens, formatDuration, formatHash } from '@/utils/format'

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

  const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')

  return (
    <Card data-testid={`stat-card-${slug}`}>
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="text-sm font-medium text-gray-400">{title}</CardTitle>
        <Icon className={`h-4 w-4 ${variantClasses[variant]}`} />
      </CardHeader>
      <CardContent>
        <div data-testid={`stat-value-${slug}`} className="text-2xl font-bold">{value}</div>
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

function DangerZone() {
  const providerInfo = useProviderInfo()
  const activeAgreements = useActiveAgreements()
  const selectedAccount = useSelectedAccount()
  const currentBlock = useBlockNumber()
  const networkId = useSelectedNetworkId()

  const [confirming, setConfirming] = useState<'announce' | 'complete' | null>(null)
  const [txStatus, setTxStatus] = useState<TxStatus | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [forceVisible, setForceVisible] = useState(false)

  const isLocal = networkId === 'local'

  // Ctrl+Shift+Alt+D toggles the force de-register button (local network only)
  useEffect(() => {
    if (!isLocal) return
    const handler = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.altKey && e.code === 'KeyD') {
        e.preventDefault()
        setForceVisible((v) => !v)
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isLocal])

  const hasAgreements = activeAgreements.length > 0
  const deregisterAt = providerInfo?.deregisterAt
  const isAnnounced = deregisterAt != null && deregisterAt > 0
  const cooldownElapsed = isAnnounced && currentBlock >= deregisterAt

  const handleProgress = useCallback((status: TxStatus) => {
    setTxStatus(status)
    if (status.type === 'error' || status.type === 'finalized') {
      setSubmitting(false)
      setConfirming(null)
    }
  }, [])

  const handleAnnounce = useCallback(async () => {
    if (!selectedAccount) return
    setSubmitting(true)
    setTxStatus(null)
    try {
      await deregisterProvider(selectedAccount, handleProgress)
    } catch {
      setSubmitting(false)
    }
  }, [selectedAccount, handleProgress])

  const handleComplete = useCallback(async () => {
    if (!selectedAccount) return
    setSubmitting(true)
    setTxStatus(null)
    try {
      await completeDeregister(selectedAccount, handleProgress)
    } catch {
      setSubmitting(false)
    }
  }, [selectedAccount, handleProgress])

  const handleForceDeregister = useCallback(async () => {
    if (!selectedAccount) return
    setSubmitting(true)
    setTxStatus(null)
    try {
      // Step 1: announce (skip if already announced)
      if (!isAnnounced) {
        setTxStatus({ type: 'signing', message: 'Step 1/2: Announcing de-registration...' })
        await deregisterProvider(selectedAccount, (s) => setTxStatus(s))
      }
      // Step 2: complete immediately
      setTxStatus({ type: 'signing', message: 'Step 2/2: Completing de-registration...' })
      await completeDeregister(selectedAccount, handleProgress)
    } catch {
      setSubmitting(false)
    }
  }, [selectedAccount, isAnnounced, handleProgress])

  return (
    <Card data-testid="danger-zone" className="border-red-500/30">
      <CardHeader>
        <div className="flex items-center gap-2">
          <AlertTriangle className="h-5 w-5 text-red-400" />
          <div>
            <CardTitle className="text-red-400">Danger Zone</CardTitle>
            <CardDescription>
              De-register your provider. This is a two-step process: announce intent, wait for the
              cooldown period, then finalize. Your stake will be returned after completion.
            </CardDescription>
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Transaction status */}
        {txStatus && (
          <div className={`flex items-center gap-2 text-sm ${
            txStatus.type === 'error' ? 'text-red-400' : 'text-gray-400'
          }`}>
            {txStatus.type !== 'error' && txStatus.type !== 'finalized' && (
              <Spinner size="sm" />
            )}
            <span>{txStatus.message}</span>
          </div>
        )}

        {/* State: has active agreements — button disabled */}
        {!isAnnounced && hasAgreements && (
          <div className="space-y-3">
            <p className="text-sm text-gray-400">
              Close all active agreements before de-registering. You currently have{' '}
              {activeAgreements.length} active agreement{activeAgreements.length > 1 ? 's' : ''}.
            </p>
            <Button variant="destructive" disabled>
              Begin De-registration
            </Button>
          </div>
        )}

        {/* State: active, no agreements — can announce */}
        {!isAnnounced && !hasAgreements && confirming !== 'announce' && (
          <Button
            variant="destructive"
            disabled={submitting}
            onClick={() => setConfirming('announce')}
          >
            Begin De-registration
          </Button>
        )}

        {/* Inline confirmation for announce */}
        {!isAnnounced && confirming === 'announce' && (
          <div className="rounded-md border border-red-500/30 bg-red-500/5 p-4 space-y-3">
            <p className="text-sm text-gray-300">
              Are you sure you want to begin de-registration? Your provider will stop accepting
              new agreements and you will need to wait for a cooldown period before finalizing.
            </p>
            <div className="flex gap-2">
              <Button
                variant="secondary"
                size="sm"
                disabled={submitting}
                onClick={() => { setConfirming(null); setTxStatus(null) }}
              >
                Cancel
              </Button>
              <Button
                variant="destructive"
                size="sm"
                disabled={submitting}
                onClick={handleAnnounce}
              >
                {submitting ? <><Spinner size="sm" /> Submitting...</> : 'Confirm De-registration'}
              </Button>
            </div>
          </div>
        )}

        {/* State: announced, cooldown pending */}
        {isAnnounced && !cooldownElapsed && (
          <div className="space-y-3">
            <div className="rounded-md border border-yellow-500/30 bg-yellow-500/5 p-3">
              <p className="text-sm text-yellow-400">
                De-registration announced. Completion available after block #{deregisterAt}.
              </p>
              <p className="text-xs text-gray-500 mt-1">
                Current block: #{currentBlock} — {deregisterAt - currentBlock} block{deregisterAt - currentBlock !== 1 ? 's' : ''} remaining
              </p>
            </div>
            <Button variant="destructive" disabled>
              Complete De-registration
            </Button>
          </div>
        )}

        {/* State: announced, cooldown elapsed — can complete */}
        {isAnnounced && cooldownElapsed && confirming !== 'complete' && (
          <div className="space-y-3">
            <div className="rounded-md border border-green-500/30 bg-green-500/5 p-3">
              <p className="text-sm text-green-400">
                Cooldown period has elapsed. You can now complete de-registration and reclaim your stake.
              </p>
            </div>
            <Button
              variant="destructive"
              disabled={submitting}
              onClick={() => setConfirming('complete')}
            >
              Complete De-registration
            </Button>
          </div>
        )}

        {/* Inline confirmation for complete */}
        {isAnnounced && cooldownElapsed && confirming === 'complete' && (
          <div className="rounded-md border border-red-500/30 bg-red-500/5 p-4 space-y-3">
            <p className="text-sm text-gray-300">
              This will permanently remove your provider and return your stake of{' '}
              {providerInfo ? formatTokens(providerInfo.stake) : '—'}. This action cannot be undone.
            </p>
            <div className="flex gap-2">
              <Button
                variant="secondary"
                size="sm"
                disabled={submitting}
                onClick={() => { setConfirming(null); setTxStatus(null) }}
              >
                Cancel
              </Button>
              <Button
                variant="destructive"
                size="sm"
                disabled={submitting}
                onClick={handleComplete}
              >
                {submitting ? <><Spinner size="sm" /> Submitting...</> : 'Confirm Removal'}
              </Button>
            </div>
          </div>
        )}

        {/* Force de-register: local dev only, hidden behind Ctrl+Shift+Alt+D */}
        {isLocal && forceVisible && (
          <div className="rounded-md border border-orange-500/30 bg-orange-500/5 p-4 space-y-3">
            <p className="text-xs font-mono text-orange-400">
              DEV ONLY — Force de-register (announce + complete in one click)
            </p>
            <Button
              variant="destructive"
              size="sm"
              disabled={submitting}
              onClick={handleForceDeregister}
            >
              {submitting ? <><Spinner size="sm" /> Forcing...</> : 'Force De-register'}
            </Button>
          </div>
        )}
      </CardContent>
    </Card>
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
  const selectedNetwork = useSelectedNetwork()
  const chainInfo = useChainInfo()
  const connectionStatus = useConnectionStatus()

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-16">
        <Spinner size="lg" />
      </div>
    )
  }

  return (
    <div data-testid="provider-info" className="space-y-6">
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

      {/* Connection */}
      <Card data-testid="provider-connection-card">
        <CardHeader className="pb-3">
          <div className="flex items-center justify-between">
            <div>
              <CardTitle>Connection</CardTitle>
              <CardDescription>
                {selectedNetwork.name}
                {selectedNetwork.isTestnet ? ' · testnet' : ''}
              </CardDescription>
            </div>
            <Network className="h-5 w-5 text-gray-400" />
          </div>
        </CardHeader>
        <CardContent>
          <NetworkStatusPanel network={selectedNetwork} />
          {connectionStatus === 'connected' && chainInfo && (
            <dl className="mt-4 grid grid-cols-1 gap-2 border-t border-gray-800 pt-4 sm:grid-cols-3">
              <div>
                <dt className="text-xs text-gray-500">Chain</dt>
                <dd data-testid="chain-name" className="text-sm font-medium">{chainInfo.name}</dd>
              </div>
              <div>
                <dt className="text-xs text-gray-500">Runtime</dt>
                <dd data-testid="chain-version" className="text-sm font-medium">{chainInfo.version}</dd>
              </div>
              <div>
                <dt className="text-xs text-gray-500">Genesis</dt>
                <dd
                  data-testid="chain-genesis"
                  title={chainInfo.genesisHash}
                  className="font-mono text-sm font-medium"
                >
                  {formatHash(chainInfo.genesisHash)}
                </dd>
              </div>
            </dl>
          )}
        </CardContent>
      </Card>

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

      {/* Danger Zone */}
      <DangerZone />
    </div>
  )
}
