import { useState } from 'react'
import { Server, Info } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { Label } from '@/components/ui/Label'
import { Switch } from '@/components/ui/Switch'
import { Spinner } from '@/components/ui/Spinner'
import {
  useProviderInfo,
  useProviderSettings,
  useIsProviderLoading,
  useIsRegistered,
  registerProvider,
  updateSettings,
  type ProviderSettings,
} from '@/state/provider.state'
import { useSelectedAccount, useSelectedBalance } from '@/state/wallet.state'
import { formatTokens, parseTokens, formatBytes } from '@/utils/format'

const UNIT = 1_000_000_000_000n

export function Registration() {
  const selectedAccount = useSelectedAccount()
  const balance = useSelectedBalance()
  const providerInfo = useProviderInfo()
  const currentSettings = useProviderSettings()
  const isLoading = useIsProviderLoading()
  const isRegistered = useIsRegistered()

  const [stake, setStake] = useState('1000')
  const [settings, setSettings] = useState<ProviderSettings>({
    minDuration: 100,
    maxDuration: 100_000,
    pricePerByte: 1_000_000n,
    acceptingPrimary: true,
    acceptingReplica: false,
    replicaSyncPrice: null,
    acceptingExtensions: true,
    maxCapacity: 1_073_741_824_000n, // 1 TB
  })
  const [error, setError] = useState<string | null>(null)

  const handleRegister = async () => {
    setError(null)
    try {
      const stakeAmount = parseTokens(stake)
      await registerProvider(stakeAmount, settings)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Registration failed')
    }
  }

  const handleUpdateSettings = async () => {
    setError(null)
    try {
      await updateSettings(settings)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Update failed')
    }
  }

  if (!selectedAccount) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Server className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Connect Your Wallet</h2>
        <p className="text-gray-500">Connect a wallet to register as a provider</p>
      </div>
    )
  }

  return (
    <div className="space-y-6 max-w-2xl mx-auto">
      <div>
        <h1 className="text-2xl font-bold">
          {isRegistered ? 'Provider Settings' : 'Provider Registration'}
        </h1>
        <p className="text-gray-400">
          {isRegistered
            ? 'Update your provider settings and configuration'
            : 'Register as a storage provider to start earning'}
        </p>
      </div>

      {error && (
        <Card className="border-red-500/50 bg-red-500/10">
          <CardContent className="py-4 text-red-400">{error}</CardContent>
        </Card>
      )}

      {/* Stake Section (only for new registration) */}
      {!isRegistered && (
        <Card>
          <CardHeader>
            <CardTitle>Stake Amount</CardTitle>
            <CardDescription>
              Minimum stake required: {formatTokens(1000n * UNIT)} (1000 tokens)
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="stake">Stake (tokens)</Label>
              <Input
                id="stake"
                type="number"
                min="1000"
                value={stake}
                onChange={(e) => setStake(e.target.value)}
                placeholder="1000"
              />
              {balance && (
                <p className="text-sm text-gray-500">
                  Available balance: {formatTokens(balance.free)}
                </p>
              )}
            </div>
            <div className="flex items-start gap-2 p-3 rounded-md bg-gray-800/50">
              <Info className="h-4 w-4 text-purple-400 mt-0.5" />
              <p className="text-sm text-gray-400">
                Your stake is locked and will be slashed if you fail to respond to challenges.
                Higher stake allows for higher capacity commitments.
              </p>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Current Stake Info (for registered providers) */}
      {isRegistered && providerInfo && (
        <Card>
          <CardHeader>
            <CardTitle>Current Stake</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-2xl font-bold">{formatTokens(providerInfo.stake)}</p>
            <p className="text-sm text-gray-500 mt-1">
              Supports up to {formatBytes(Number(providerInfo.capacity))} capacity
            </p>
          </CardContent>
        </Card>
      )}

      {/* Settings Section */}
      <Card>
        <CardHeader>
          <CardTitle>Provider Settings</CardTitle>
          <CardDescription>Configure your pricing and acceptance settings</CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {/* Pricing */}
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="pricePerByte">Price per Byte (per block)</Label>
              <Input
                id="pricePerByte"
                type="number"
                value={settings.pricePerByte.toString()}
                onChange={(e) =>
                  setSettings({ ...settings, pricePerByte: BigInt(e.target.value || '0') })
                }
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="replicaSyncPrice">Replica Sync Price (optional)</Label>
              <Input
                id="replicaSyncPrice"
                type="number"
                value={settings.replicaSyncPrice?.toString() || ''}
                onChange={(e) =>
                  setSettings({
                    ...settings,
                    replicaSyncPrice: e.target.value ? BigInt(e.target.value) : null,
                  })
                }
                placeholder="Optional"
              />
            </div>
          </div>

          {/* Duration */}
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="minDuration">Min Duration (blocks)</Label>
              <Input
                id="minDuration"
                type="number"
                min="1"
                value={settings.minDuration}
                onChange={(e) =>
                  setSettings({ ...settings, minDuration: parseInt(e.target.value) || 1 })
                }
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="maxDuration">Max Duration (blocks)</Label>
              <Input
                id="maxDuration"
                type="number"
                min="1"
                value={settings.maxDuration}
                onChange={(e) =>
                  setSettings({ ...settings, maxDuration: parseInt(e.target.value) || 1 })
                }
              />
            </div>
          </div>

          {/* Capacity */}
          <div className="space-y-2">
            <Label htmlFor="maxCapacity">Max Capacity (bytes)</Label>
            <Input
              id="maxCapacity"
              type="number"
              value={settings.maxCapacity.toString()}
              onChange={(e) =>
                setSettings({ ...settings, maxCapacity: BigInt(e.target.value || '0') })
              }
            />
            <p className="text-sm text-gray-500">
              {formatBytes(Number(settings.maxCapacity))} total capacity
            </p>
          </div>

          {/* Toggles */}
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <div>
                <Label>Accept Primary Agreements</Label>
                <p className="text-sm text-gray-500">Accept new primary storage agreements</p>
              </div>
              <Switch
                checked={settings.acceptingPrimary}
                onCheckedChange={(checked) =>
                  setSettings({ ...settings, acceptingPrimary: checked })
                }
              />
            </div>
            <div className="flex items-center justify-between">
              <div>
                <Label>Accept Replica Agreements</Label>
                <p className="text-sm text-gray-500">Accept replica/backup storage requests</p>
              </div>
              <Switch
                checked={settings.acceptingReplica}
                onCheckedChange={(checked) =>
                  setSettings({ ...settings, acceptingReplica: checked })
                }
              />
            </div>
            <div className="flex items-center justify-between">
              <div>
                <Label>Accept Extensions</Label>
                <p className="text-sm text-gray-500">Allow existing agreements to be extended</p>
              </div>
              <Switch
                checked={settings.acceptingExtensions}
                onCheckedChange={(checked) =>
                  setSettings({ ...settings, acceptingExtensions: checked })
                }
              />
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Submit Button */}
      <Button
        className="w-full"
        size="lg"
        onClick={isRegistered ? handleUpdateSettings : handleRegister}
        disabled={isLoading}
      >
        {isLoading ? (
          <>
            <Spinner size="sm" className="mr-2" />
            {isRegistered ? 'Updating...' : 'Registering...'}
          </>
        ) : isRegistered ? (
          'Update Settings'
        ) : (
          'Register as Provider'
        )}
      </Button>
    </div>
  )
}
