/**
 * Registration Page
 *
 * Handles both:
 * 1. New provider registration (wizard flow)
 * 2. Existing provider settings management
 *
 * The flow detects whether the connected wallet is already registered
 * and shows the appropriate UI.
 */

import { useState, useEffect } from 'react'
import {
  Info,
  Wallet,
  CheckCircle,
  ArrowRight,
  ArrowLeft,
  AlertCircle,
  ExternalLink,
  Globe,
} from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { Label } from '@/components/ui/Label'
import { Switch } from '@/components/ui/Switch'
import { Badge } from '@/components/ui/Badge'
import { Spinner } from '@/components/ui/Spinner'
import { Progress } from '@/components/ui/Progress'
import {
  useSelectedAccount,
  useSelectedBalance,
  useIsSelectedRegistered,
  useWalletStatus,
  refreshExtensions,
  connectExtension,
  markAsRegistered,
} from '@/state/wallet.state'
import { useIsConnected, connect as connectChain } from '@/state/chain.state'
import {
  useProviderInfo,
  useProviderSettings,
  useIsProviderLoading,
  updateSettings,
  updateMultiaddr,
  loadProviderData,
  registerProvider,
  type ProviderSettings,
  type TxStatus,
} from '@/state/provider.state'
import { formatTokens, parseTokens, formatBytes } from '@/utils/format'

// Min stake is fetched from chain constants at connection time.
// Falls back to 1000 tokens if not yet loaded.
function getMinStake(): bigint {
  return (globalThis as any).__minProviderStake ?? 1_000_000_000_000_000n
}

// Mirror of the runtime's `MinStakePerByte` constant (raw units / byte).
// The runtime rejects register_provider with `InsufficientStakeForCapacity`
// if `stake < max_capacity * MIN_STAKE_PER_BYTE`. Hardcoded for now;
// could be plumbed from chain consts in a follow-up.
const MIN_STAKE_PER_BYTE = 1_000n

function requiredStakeForCapacity(maxCapacity: bigint): bigint {
  return maxCapacity * MIN_STAKE_PER_BYTE
}

// Registration wizard steps
type WizardStep = 'connect' | 'stake' | 'settings' | 'confirm' | 'complete'

const WIZARD_STEPS: { id: WizardStep; title: string }[] = [
  { id: 'connect', title: 'Connect' },
  { id: 'stake', title: 'Stake' },
  { id: 'settings', title: 'Settings' },
  { id: 'confirm', title: 'Confirm' },
  { id: 'complete', title: 'Complete' },
]

export function Registration() {
  const selectedAccount = useSelectedAccount()
  const balance = useSelectedBalance()
  const isRegistered = useIsSelectedRegistered()
  const walletStatus = useWalletStatus()
  const isChainConnected = useIsConnected()

  // Wizard state
  const [step, setStep] = useState<WizardStep>('connect')
  const [stake, setStake] = useState('1000')
  const [multiaddr, setMultiaddr] = useState('/ip4/127.0.0.1/tcp/3000')
  const [settings, setSettings] = useState<ProviderSettings>({
    minDuration: 100,
    maxDuration: 100_000,
    pricePerByte: 1_000_000n,
    acceptingPrimary: true,
    acceptingReplica: false,
    replicaSyncPrice: null,
    acceptingExtensions: true,
    maxCapacity: 1_099_511_627_776n, // 1 TB (2^40 bytes)
  })
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [txHash, setTxHash] = useState<string | null>(null)
  const [progressStatus, setProgressStatus] = useState<string | null>(null)

  // Auto-advance wizard based on state
  useEffect(() => {
    if (step === 'connect') {
      if (isChainConnected && selectedAccount) {
        setStep('stake')
      }
    }
  }, [step, isChainConnected, selectedAccount])

  // Load existing settings if registered
  useEffect(() => {
    if (isRegistered && selectedAccount) {
      loadProviderData(selectedAccount.address)
    }
  }, [isRegistered, selectedAccount])

  // If registered, show settings management instead of wizard
  if (isRegistered === true) {
    return <SettingsManager />
  }

  // Show loading while checking registration
  if (isRegistered === null && selectedAccount) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Spinner size="lg" />
        <p className="text-gray-400 mt-4">Checking registration status...</p>
      </div>
    )
  }

  // Calculate progress
  const currentStepIndex = WIZARD_STEPS.findIndex((s) => s.id === step)
  const progress = ((currentStepIndex + 1) / WIZARD_STEPS.length) * 100

  const handleConnect = async () => {
    setError(null)
    try {
      if (!isChainConnected) {
        await connectChain()
      }

      // Refresh and connect to wallet extension
      const available = refreshExtensions()
      if (available.length === 0) {
        setError('No wallet extensions found. Please install Polkadot.js, Talisman, or SubWallet.')
        return
      }

      // Connect to first available extension (or could show picker)
      await connectExtension(available[0])
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Connection failed')
    }
  }

  const handleRegister = async () => {
    if (!selectedAccount) {
      setError('No account selected')
      return
    }

    setIsSubmitting(true)
    setError(null)
    setProgressStatus('Preparing transaction...')

    try {
      const stakeAmount = parseTokens(stake)

      // Submit the registration extrinsic via wallet signer with progress callback
      await registerProvider(stakeAmount, multiaddr, settings, selectedAccount, (status: TxStatus) => {
        setProgressStatus(status.message)
        if (status.type === 'inBlock' || status.type === 'finalized') {
          setTxHash('blockHash' in status ? status.blockHash : null)
        }
      })

      // Mark as registered in state
      markAsRegistered(selectedAccount.address)
      setProgressStatus(null)
      setStep('complete')
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Registration failed')
      setProgressStatus(null)
    } finally {
      setIsSubmitting(false)
    }
  }

  const stakeAmount = parseTokens(stake)
  const hasEnoughBalance = balance && balance.free >= stakeAmount
  const meetsMinStake = stakeAmount >= getMinStake()

  return (
    <div className="max-w-2xl mx-auto space-y-6">
      {/* Header */}
      <div>
        <h1 className="text-2xl font-bold">Become a Storage Provider</h1>
        <p className="text-gray-400">
          Register on-chain to start accepting storage agreements and earning rewards
        </p>
      </div>

      {/* Progress */}
      <div className="space-y-2">
        <div className="flex justify-between text-sm">
          {WIZARD_STEPS.map((s, i) => (
            <span
              key={s.id}
              className={
                i <= currentStepIndex ? 'text-purple-400' : 'text-gray-600'
              }
            >
              {s.title}
            </span>
          ))}
        </div>
        <Progress value={progress} />
      </div>

      {/* Error */}
      {error && (
        <Card className="border-red-500/50 bg-red-500/10">
          <CardContent className="flex items-center gap-3 py-4">
            <AlertCircle className="h-5 w-5 text-red-500" />
            <p className="text-red-400">{error}</p>
          </CardContent>
        </Card>
      )}

      {/* Step: Connect */}
      {step === 'connect' && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Wallet className="h-5 w-5" />
              Connect to Get Started
            </CardTitle>
            <CardDescription>
              Connect your wallet and the blockchain to begin registration
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            <div className="space-y-4">
              <div className="flex items-center justify-between p-4 rounded-lg bg-gray-800/50">
                <div className="flex items-center gap-3">
                  <div
                    className={`h-3 w-3 rounded-full ${
                      isChainConnected ? 'bg-green-500' : 'bg-gray-600'
                    }`}
                  />
                  <span>Blockchain Connection</span>
                </div>
                {isChainConnected ? (
                  <Badge variant="success">Connected</Badge>
                ) : (
                  <Badge variant="secondary">Disconnected</Badge>
                )}
              </div>

              <div className="flex items-center justify-between p-4 rounded-lg bg-gray-800/50">
                <div className="flex items-center gap-3">
                  <div
                    className={`h-3 w-3 rounded-full ${
                      selectedAccount ? 'bg-green-500' : 'bg-gray-600'
                    }`}
                  />
                  <span>Wallet</span>
                </div>
                {selectedAccount ? (
                  <Badge variant="success">{selectedAccount.name}</Badge>
                ) : (
                  <Badge variant="secondary">Not Connected</Badge>
                )}
              </div>
            </div>

            <Button
              className="w-full"
              size="lg"
              onClick={handleConnect}
              disabled={walletStatus === 'connecting'}
            >
              {walletStatus === 'connecting' ? (
                <>
                  <Spinner size="sm" className="mr-2" />
                  Connecting...
                </>
              ) : (
                <>
                  Connect Wallet
                  <ArrowRight className="ml-2 h-4 w-4" />
                </>
              )}
            </Button>
          </CardContent>
        </Card>
      )}

      {/* Step: Stake */}
      {step === 'stake' && (
        <Card>
          <CardHeader>
            <CardTitle>Set Your Stake</CardTitle>
            <CardDescription>
              Stake tokens as collateral. Higher stake enables more capacity.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            <div className="space-y-2">
              <Label htmlFor="stake">Stake Amount (tokens)</Label>
              <Input
                id="stake"
                data-testid="registration-stake-input"
                type="number"
                min="1000"
                value={stake}
                onChange={(e) => setStake(e.target.value)}
                placeholder="1000"
              />
              <div className="flex justify-between text-sm">
                <span className="text-gray-500">
                  Minimum: {formatTokens(getMinStake())}
                </span>
                {balance && (
                  <span className="text-gray-500">
                    Available: {formatTokens(balance.free)}
                  </span>
                )}
              </div>
            </div>

            {!meetsMinStake && (
              <div className="flex items-start gap-2 p-3 rounded-md bg-yellow-500/10 border border-yellow-500/30">
                <AlertCircle className="h-4 w-4 text-yellow-500 mt-0.5" />
                <p className="text-sm text-yellow-400">
                  Stake must be at least {formatTokens(getMinStake())}
                </p>
              </div>
            )}

            {meetsMinStake && !hasEnoughBalance && (
              <div className="flex items-start gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/30">
                <AlertCircle className="h-4 w-4 text-red-500 mt-0.5" />
                <p className="text-sm text-red-400">
                  Insufficient balance. You need {formatTokens(stakeAmount)} but only have{' '}
                  {balance ? formatTokens(balance.free) : '0'}
                </p>
              </div>
            )}

            <div className="flex items-start gap-2 p-3 rounded-md bg-gray-800/50">
              <Info className="h-4 w-4 text-purple-400 mt-0.5" />
              <div className="text-sm text-gray-400">
                <p>Your stake serves as collateral and will be:</p>
                <ul className="list-disc list-inside mt-1 space-y-1">
                  <li>Locked while you have active agreements</li>
                  <li>Slashed if you fail to respond to challenges</li>
                  <li>Returnable when you deregister (after cooldown)</li>
                </ul>
              </div>
            </div>

            <div className="flex gap-3">
              <Button variant="outline" onClick={() => setStep('connect')}>
                <ArrowLeft className="mr-2 h-4 w-4" />
                Back
              </Button>
              <Button
                data-testid="registration-stake-continue"
                className="flex-1"
                onClick={() => setStep('settings')}
                disabled={!meetsMinStake || !hasEnoughBalance}
              >
                Continue
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Step: Settings */}
      {step === 'settings' && (
        <Card>
          <CardHeader>
            <CardTitle>Configure Provider Settings</CardTitle>
            <CardDescription>Set your pricing and acceptance preferences</CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Provider Node Address */}
            <div className="space-y-2">
              <Label htmlFor="multiaddr">Provider Node Address (multiaddr)</Label>
              <Input
                id="multiaddr"
                data-testid="registration-multiaddr-input"
                type="text"
                value={multiaddr}
                onChange={(e) => setMultiaddr(e.target.value)}
                placeholder="/ip4/127.0.0.1/tcp/3000"
              />
              <p className="text-xs text-gray-500">
                The address where your provider node will accept connections (e.g., /ip4/127.0.0.1/tcp/3000)
              </p>
            </div>

            {/* Pricing */}
            <div className="grid gap-4 md:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="pricePerByte">Price per Byte (per block)</Label>
                <Input
                  id="pricePerByte"
                  data-testid="registration-priceperbyte-input"
                  type="number"
                  value={settings.pricePerByte.toString()}
                  onChange={(e) =>
                    setSettings({ ...settings, pricePerByte: BigInt(e.target.value || '0') })
                  }
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="maxCapacity">Max Capacity (bytes)</Label>
                <Input
                  id="maxCapacity"
                  data-testid="registration-maxcapacity-input"
                  type="number"
                  value={settings.maxCapacity.toString()}
                  onChange={(e) =>
                    setSettings({ ...settings, maxCapacity: BigInt(e.target.value || '0') })
                  }
                />
                <p className="text-xs text-gray-500">
                  {formatBytes(Number(settings.maxCapacity))}
                </p>
              </div>
            </div>

            {/* Duration */}
            <div className="grid gap-4 md:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="minDuration">Min Duration (blocks)</Label>
                <Input
                  id="minDuration"
                  data-testid="registration-minduration-input"
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
                  data-testid="registration-maxduration-input"
                  type="number"
                  min="1"
                  value={settings.maxDuration}
                  onChange={(e) =>
                    setSettings({ ...settings, maxDuration: parseInt(e.target.value) || 1 })
                  }
                />
              </div>
            </div>

            {/* Toggles */}
            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <Label>Accept Primary Agreements</Label>
                  <p className="text-sm text-gray-500">Be the primary storage provider</p>
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
                  <p className="text-sm text-gray-500">Store backup copies</p>
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
                  <p className="text-sm text-gray-500">Allow agreement extensions</p>
                </div>
                <Switch
                  checked={settings.acceptingExtensions}
                  onCheckedChange={(checked) =>
                    setSettings({ ...settings, acceptingExtensions: checked })
                  }
                />
              </div>
            </div>

            <div className="flex gap-3">
              <Button variant="outline" onClick={() => setStep('stake')}>
                <ArrowLeft className="mr-2 h-4 w-4" />
                Back
              </Button>
              <Button data-testid="registration-settings-continue" className="flex-1" onClick={() => setStep('confirm')}>
                Review & Confirm
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Step: Confirm */}
      {step === 'confirm' && (
        <Card>
          <CardHeader>
            <CardTitle>Confirm Registration</CardTitle>
            <CardDescription>Review your settings before submitting</CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Summary */}
            <div className="space-y-4 p-4 rounded-lg bg-gray-800/50">
              <div className="flex justify-between">
                <span className="text-gray-400">Account</span>
                <span className="font-mono">{selectedAccount?.name}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-400">Stake</span>
                <span className="font-medium">{formatTokens(parseTokens(stake))}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-400">Node Address</span>
                <span className="font-mono text-sm">{multiaddr}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-400">Price per Byte</span>
                <span>{formatTokens(settings.pricePerByte)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-400">Max Capacity</span>
                <span>{formatBytes(Number(settings.maxCapacity))}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-400">Duration Range</span>
                <span>
                  {settings.minDuration.toLocaleString()} - {settings.maxDuration.toLocaleString()} blocks
                </span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-400">Accepting</span>
                <div className="flex gap-2">
                  {settings.acceptingPrimary && <Badge>Primary</Badge>}
                  {settings.acceptingReplica && <Badge>Replica</Badge>}
                  {settings.acceptingExtensions && <Badge variant="secondary">Extensions</Badge>}
                </div>
              </div>
            </div>

            <div className="flex items-start gap-2 p-3 rounded-md bg-purple-500/10 border border-purple-500/30">
              <Info className="h-4 w-4 text-purple-400 mt-0.5" />
              <p className="text-sm text-purple-300">
                This will submit a transaction to the blockchain. You'll need to sign it with your
                wallet.
              </p>
            </div>

            {stakeAmount < requiredStakeForCapacity(settings.maxCapacity) && (
              <div
                data-testid="registration-stake-too-low"
                className="flex items-start gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/30"
              >
                <AlertCircle className="h-4 w-4 text-red-500 mt-0.5" />
                <p className="text-sm text-red-400">
                  Stake of {formatTokens(stakeAmount)} is insufficient for{' '}
                  {formatBytes(Number(settings.maxCapacity))} capacity. The runtime requires at
                  least {formatTokens(requiredStakeForCapacity(settings.maxCapacity))}. Go back and
                  raise the stake or lower the capacity.
                </p>
              </div>
            )}

            <div className="flex gap-3">
              <Button variant="outline" onClick={() => setStep('settings')}>
                <ArrowLeft className="mr-2 h-4 w-4" />
                Back
              </Button>
              <Button
                data-testid="registration-submit"
                className="flex-1"
                onClick={handleRegister}
                disabled={
                  isSubmitting ||
                  stakeAmount < requiredStakeForCapacity(settings.maxCapacity)
                }
              >
                {isSubmitting ? (
                  <>
                    <Spinner size="sm" className="mr-2" />
                    {progressStatus || 'Registering...'}
                  </>
                ) : (
                  <>
                    Register Provider
                    <CheckCircle className="ml-2 h-4 w-4" />
                  </>
                )}
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Step: Complete */}
      {step === 'complete' && (
        <Card data-testid="registration-complete" className="border-green-500/30">
          <CardContent className="py-12 text-center space-y-6">
            <div className="flex justify-center">
              <div className="h-16 w-16 rounded-full bg-green-500/20 flex items-center justify-center">
                <CheckCircle className="h-8 w-8 text-green-500" />
              </div>
            </div>
            <div>
              <h2 className="text-xl font-bold text-green-400">Registration Successful!</h2>
              <p className="text-gray-400 mt-2">
                You are now registered as a storage provider on-chain.
              </p>
            </div>

            {txHash && (
              <div className="p-3 rounded-lg bg-gray-800/50">
                <p className="text-sm text-gray-400">Transaction Hash</p>
                <code className="text-xs text-purple-400">{txHash}</code>
              </div>
            )}

            <div className="space-y-3">
              <h3 className="font-medium">Next Steps</h3>
              <div className="text-left space-y-2 text-sm text-gray-400">
                <div className="flex items-start gap-2">
                  <span className="text-purple-400">1.</span>
                  <span>Set up and run your provider node</span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-purple-400">2.</span>
                  <span>Configure your provider node with your keypair</span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-purple-400">3.</span>
                  <span>Start accepting storage agreements</span>
                </div>
              </div>
            </div>

            <div className="flex gap-3 justify-center">
              <Button variant="outline" asChild>
                <a
                  href="https://github.com/user/web3-storage/blob/main/docs/getting-started/LAYER1_QUICKSTART.md"
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  View Setup Guide
                  <ExternalLink className="ml-2 h-4 w-4" />
                </a>
              </Button>
              <Button asChild>
                <a href="/">Go to Dashboard</a>
              </Button>
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  )
}

// ─────────────────────────────────────────────────────────────────────────────
// Settings Manager (for already registered providers)
// ─────────────────────────────────────────────────────────────────────────────

function SettingsManager() {
  const selectedAccount = useSelectedAccount()
  const providerInfo = useProviderInfo()
  const currentSettings = useProviderSettings()
  const isLoading = useIsProviderLoading()

  const [settings, setSettings] = useState<ProviderSettings | null>(null)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState(false)
  const [progressStatus, setProgressStatus] = useState<string | null>(null)

  // Multiaddr update state
  const [multiaddrValue, setMultiaddrValue] = useState('')
  const [isMultiaddrSubmitting, setIsMultiaddrSubmitting] = useState(false)
  const [multiaddrError, setMultiaddrError] = useState<string | null>(null)
  const [multiaddrSuccess, setMultiaddrSuccess] = useState(false)
  const [multiaddrProgress, setMultiaddrProgress] = useState<string | null>(null)

  // Initialize form with current settings
  useEffect(() => {
    if (currentSettings) {
      setSettings(currentSettings)
    }
  }, [currentSettings])

  // Initialize multiaddr from provider info
  useEffect(() => {
    if (providerInfo?.multiaddr) {
      setMultiaddrValue(providerInfo.multiaddr)
    }
  }, [providerInfo?.multiaddr])

  const isMultiaddrValid = (addr: string) => {
    return (addr.startsWith('/ip4/') || addr.startsWith('/dns4/')) && addr.includes('/tcp/')
  }

  const handleUpdateMultiaddr = async () => {
    if (!selectedAccount || !multiaddrValue) return

    if (!isMultiaddrValid(multiaddrValue)) {
      setMultiaddrError('Multiaddr must start with /ip4/ or /dns4/ and contain /tcp/')
      return
    }

    setIsMultiaddrSubmitting(true)
    setMultiaddrError(null)
    setMultiaddrSuccess(false)
    setMultiaddrProgress('Preparing transaction...')

    try {
      await updateMultiaddr(multiaddrValue, selectedAccount, (status: TxStatus) => {
        setMultiaddrProgress(status.message)
      })
      setMultiaddrSuccess(true)
      setMultiaddrProgress(null)
      setTimeout(() => setMultiaddrSuccess(false), 3000)
    } catch (err) {
      setMultiaddrError(err instanceof Error ? err.message : 'Update failed')
      setMultiaddrProgress(null)
    } finally {
      setIsMultiaddrSubmitting(false)
    }
  }

  const handleUpdate = async () => {
    if (!settings || !selectedAccount) return

    setIsSubmitting(true)
    setError(null)
    setSuccess(false)
    setProgressStatus('Preparing transaction...')

    try {
      await updateSettings(settings, selectedAccount, (status: TxStatus) => {
        setProgressStatus(status.message)
      })
      setSuccess(true)
      setProgressStatus(null)
      setTimeout(() => setSuccess(false), 3000)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Update failed')
      setProgressStatus(null)
    } finally {
      setIsSubmitting(false)
    }
  }

  if (isLoading || !settings) {
    return (
      <div className="flex items-center justify-center py-16">
        <Spinner size="lg" />
      </div>
    )
  }

  return (
    <div className="max-w-2xl mx-auto space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Provider Settings</h1>
          <p className="text-gray-400">Update your provider configuration</p>
        </div>
        <Badge data-testid="provider-registered-badge" variant="success">Registered</Badge>
      </div>

      {error && (
        <Card className="border-red-500/50 bg-red-500/10">
          <CardContent className="py-4 text-red-400">{error}</CardContent>
        </Card>
      )}

      {success && (
        <Card className="border-green-500/50 bg-green-500/10">
          <CardContent className="flex items-center gap-3 py-4">
            <CheckCircle className="h-5 w-5 text-green-500" />
            <p className="text-green-400">Settings updated successfully!</p>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Globe className="h-5 w-5" />
            Node Address
          </CardTitle>
          <CardDescription>
            The multiaddr where your provider node accepts connections
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {multiaddrError && (
            <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/30">
              <AlertCircle className="h-4 w-4 text-red-500" />
              <p className="text-sm text-red-400">{multiaddrError}</p>
            </div>
          )}

          {multiaddrSuccess && (
            <div className="flex items-center gap-2 p-3 rounded-md bg-green-500/10 border border-green-500/30">
              <CheckCircle className="h-4 w-4 text-green-500" />
              <p className="text-sm text-green-400">Multiaddr updated successfully!</p>
            </div>
          )}

          <div className="space-y-2">
            <Label htmlFor="multiaddr">Multiaddr</Label>
            <div className="flex gap-3">
              <Input
                id="multiaddr"
                data-testid="settings-multiaddr-input"
                type="text"
                value={multiaddrValue}
                onChange={(e) => setMultiaddrValue(e.target.value)}
                placeholder="/ip4/127.0.0.1/tcp/3000"
                className="flex-1"
              />
              <Button
                data-testid="settings-multiaddr-update"
                onClick={handleUpdateMultiaddr}
                disabled={isMultiaddrSubmitting || !multiaddrValue || multiaddrValue === providerInfo?.multiaddr}
              >
                {isMultiaddrSubmitting ? (
                  <>
                    <Spinner size="sm" className="mr-2" />
                    {multiaddrProgress || 'Updating...'}
                  </>
                ) : (
                  'Update'
                )}
              </Button>
            </div>
            <p className="text-xs text-gray-500">
              Must start with /ip4/ or /dns4/ and contain /tcp/ (e.g., /ip4/127.0.0.1/tcp/3000)
            </p>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Pricing & Capacity</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="pricePerByte">Price per Byte (per block)</Label>
              <Input
                id="pricePerByte"
                data-testid="settings-priceperbyte-input"
                type="number"
                className={settings.pricePerByte === 0n ? 'text-gray-500' : ''}
                value={settings.pricePerByte.toString()}
                onChange={(e) =>
                  setSettings({ ...settings, pricePerByte: BigInt(e.target.value || '0') })
                }
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="maxCapacity">Max Capacity</Label>
              <Input
                id="maxCapacity"
                data-testid="settings-maxcapacity-input"
                type="number"
                className={settings.maxCapacity === 0n ? 'text-gray-500' : ''}
                value={settings.maxCapacity.toString()}
                onChange={(e) =>
                  setSettings({ ...settings, maxCapacity: BigInt(e.target.value || '0') })
                }
              />
              <p className="text-xs text-gray-500">{settings.maxCapacity === 0n ? '0 = unlimited' : formatBytes(Number(settings.maxCapacity))}</p>
            </div>
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="minDuration">Min Duration (blocks)</Label>
              <Input
                id="minDuration"
                type="number"
                className={settings.minDuration === 0 ? 'text-gray-500' : ''}
                value={settings.minDuration}
                onChange={(e) =>
                  setSettings({ ...settings, minDuration: parseInt(e.target.value) || 0 })
                }
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="maxDuration">Max Duration (blocks)</Label>
              <Input
                id="maxDuration"
                type="number"
                className={settings.maxDuration >= 4_000_000_000 ? 'text-gray-500' : ''}
                value={settings.maxDuration}
                onChange={(e) =>
                  setSettings({ ...settings, maxDuration: parseInt(e.target.value) || 1 })
                }
              />
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Acceptance Settings</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex items-center justify-between">
            <div>
              <Label>Accept Primary Agreements</Label>
              <p className="text-sm text-gray-500">Be the primary storage provider</p>
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
              <p className="text-sm text-gray-500">Store backup copies</p>
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
              <p className="text-sm text-gray-500">Allow agreement extensions</p>
            </div>
            <Switch
              checked={settings.acceptingExtensions}
              onCheckedChange={(checked) =>
                setSettings({ ...settings, acceptingExtensions: checked })
              }
            />
          </div>
        </CardContent>
      </Card>

      <Button
        data-testid="settings-update"
        className="w-full"
        size="lg"
        onClick={handleUpdate}
        disabled={isSubmitting}
      >
        {isSubmitting ? (
          <>
            <Spinner size="sm" className="mr-2" />
            {progressStatus || 'Updating...'}
          </>
        ) : (
          'Update Settings'
        )}
      </Button>
    </div>
  )
}
