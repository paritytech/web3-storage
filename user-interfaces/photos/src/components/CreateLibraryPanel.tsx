// SPDX-License-Identifier: GPL-3.0-only
//
// M5 — the interactive State A body. Lists on-chain providers, takes size +
// duration inputs, previews the cost (in tokens), and drives the
// map → negotiate → createLibrary flow. On success it calls `onCreated`, which
// re-reads `libraryOf` and flips the page to State B.

import { useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, Check, FolderPlus, HardDrive } from 'lucide-react'
import type { InjectedPolkadotAccount } from 'polkadot-api/pjs-signer'
import type { NetworkConfig } from '@web3-storage/network-config'
import type { ResolvedContract } from '@/lib/photos-contract'
import { computePaymentAndValue } from '@/lib/photos-contract-write'
import { annotate, type PhotosProvider } from '@/lib/photos-providers'
import {
  createLibrary,
  loadProviders,
  resetCreation,
  retryCreate,
  useCreation,
  useProviders,
  useProvidersError,
  useProvidersLoading,
} from '@/state/library.state'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { Spinner } from '@/components/ui/Spinner'
import { Badge } from '@/components/ui/Badge'
import { bytesFromUnit, formatAddress, formatBytes, formatTokens, type ByteUnit } from '@/utils/format'

interface Props {
  account: InjectedPolkadotAccount
  contract: ResolvedContract
  network: NetworkConfig
  onCreated: () => void
}

const STAGE_LABEL: Record<string, string> = {
  mapping: 'Registering your account…',
  negotiating: 'Negotiating terms with the provider…',
  submitting: 'Creating your library on-chain…',
  ready: 'Library created — loading…',
}

export function CreateLibraryPanel({ account, contract, network, onCreated }: Props) {
  const providers = useProviders()
  const loading = useProvidersLoading()
  const providersError = useProvidersError()
  const creation = useCreation()

  const [size, setSize] = useState('1')
  const [unit, setUnit] = useState<ByteUnit>('MiB')
  const [duration, setDuration] = useState('50')
  const [name, setName] = useState('my-photos')

  // Reset any prior creation state and (re)load providers for this account/network.
  useEffect(() => {
    resetCreation()
    void loadProviders()
  }, [account.address, network.id])

  // Flip to State B once the library exists on-chain. Fire exactly once per
  // transition into `ready` (via a ref) so a parent re-render can't re-trigger it.
  const onCreatedRef = useRef(onCreated)
  onCreatedRef.current = onCreated
  useEffect(() => {
    if (creation.stage === 'ready') onCreatedRef.current()
  }, [creation.stage])

  const sizeBytes = useMemo(() => {
    const n = Number(size)
    return Number.isFinite(n) && n > 0 ? bytesFromUnit(n, unit) : 0n
  }, [size, unit])
  const durationBlocks = useMemo(() => {
    const n = Number(duration)
    return Number.isInteger(n) && n > 0 ? n : 0
  }, [duration])

  const inFlight =
    creation.stage === 'mapping' ||
    creation.stage === 'negotiating' ||
    creation.stage === 'submitting' ||
    creation.stage === 'ready'

  const inputsValid = sizeBytes > 0n && durationBlocks > 0 && name.trim().length > 0

  function handleCreate(provider: PhotosProvider) {
    void createLibrary({
      account,
      contract,
      provider,
      sizeBytes,
      durationBlocks,
      name: name.trim(),
    })
  }

  return (
    <div className="max-w-2xl mx-auto">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <FolderPlus className="h-5 w-5 text-purple-500" /> Create your library
          </CardTitle>
          <CardDescription>
            {account.name || formatAddress(account.address)} — pick a storage provider and open an
            agreement on {network.name}.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-5">
          {/* ── Size / duration / name inputs ── */}
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label="Storage size">
              <div className="flex items-center gap-2">
                <Input
                  type="number"
                  min="0"
                  step="any"
                  value={size}
                  disabled={inFlight}
                  onChange={(e) => setSize(e.target.value)}
                  data-testid="size-input"
                />
                <select
                  value={unit}
                  disabled={inFlight}
                  onChange={(e) => setUnit(e.target.value as ByteUnit)}
                  className="h-9 rounded-md border border-gray-700 bg-gray-900 px-2 text-sm text-gray-200 focus:outline-none focus:ring-1 focus:ring-purple-500 disabled:opacity-50"
                >
                  <option value="MiB">MiB</option>
                  <option value="GiB">GiB</option>
                </select>
              </div>
            </Field>
            <Field label="Duration (blocks)">
              <Input
                type="number"
                min="1"
                step="1"
                value={duration}
                disabled={inFlight}
                onChange={(e) => setDuration(e.target.value)}
                data-testid="duration-input"
              />
            </Field>
          </div>
          <Field label="Library name">
            <Input
              value={name}
              disabled={inFlight}
              onChange={(e) => setName(e.target.value)}
              placeholder="my-photos"
              data-testid="name-input"
            />
          </Field>

          {/* ── Status (in-flight / failed) vs provider list ── */}
          {creation.stage === 'failed' ? (
            <FailureCard message={creation.error?.message ?? 'Something went wrong.'} />
          ) : inFlight ? (
            <div
              className="flex items-center gap-3 rounded-md border border-gray-800 bg-gray-900/40 p-4 text-sm text-gray-300"
              data-testid="creation-status"
            >
              {creation.stage === 'ready' ? (
                <Check className="h-5 w-5 text-green-400" />
              ) : (
                <Spinner size="sm" />
              )}
              {STAGE_LABEL[creation.stage]}
            </div>
          ) : (
            <ProviderList
              providers={providers}
              loading={loading}
              error={providersError}
              inputsValid={inputsValid}
              sizeBytes={sizeBytes}
              durationBlocks={durationBlocks}
              onRetryLoad={() => void loadProviders()}
              onCreate={handleCreate}
            />
          )}

          <p className="text-[11px] text-gray-600">
            Photos contract <code>{formatAddress(contract.address, 6)}</code> ({contract.source})
          </p>
        </CardContent>
      </Card>
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block space-y-1.5">
      <span className="text-xs font-medium text-gray-400">{label}</span>
      {children}
    </label>
  )
}

function FailureCard({ message }: { message: string }) {
  return (
    <div className="space-y-3 rounded-md border border-red-900/50 bg-red-950/20 p-4">
      <div className="flex items-start gap-2 text-sm text-red-300">
        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
        <span data-testid="creation-error">{message}</span>
      </div>
      <div className="flex items-center gap-2">
        <Button size="sm" onClick={() => void retryCreate()}>
          Try again
        </Button>
        <Button size="sm" variant="outline" onClick={() => resetCreation()}>
          Back to providers
        </Button>
      </div>
    </div>
  )
}

function ProviderList({
  providers,
  loading,
  error,
  inputsValid,
  sizeBytes,
  durationBlocks,
  onRetryLoad,
  onCreate,
}: {
  providers: PhotosProvider[]
  loading: boolean
  error?: string
  inputsValid: boolean
  sizeBytes: bigint
  durationBlocks: number
  onRetryLoad: () => void
  onCreate: (provider: PhotosProvider) => void
}) {
  if (loading) {
    return (
      <div className="flex items-center gap-3 p-4 text-sm text-gray-400">
        <Spinner size="sm" /> Looking for providers…
      </div>
    )
  }
  if (error) {
    return (
      <div className="space-y-2 rounded-md border border-red-900/50 bg-red-950/20 p-4 text-sm text-red-300">
        <div className="flex items-start gap-2">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" /> {error}
        </div>
        <Button size="sm" variant="outline" onClick={onRetryLoad}>
          Retry
        </Button>
      </div>
    )
  }
  if (providers.length === 0) {
    return (
      <div className="rounded-md border border-gray-800 bg-gray-900/40 p-4 text-sm text-gray-400">
        No storage providers are registered on this network yet.
      </div>
    )
  }

  return (
    <div className="space-y-2">
      <div className="text-xs font-medium text-gray-400">Storage providers</div>
      {providers.map((p) => {
        const { eligible, reasons } = annotate(p, { bytesNeeded: sizeBytes, durationBlocks })
        const { payment, value } = inputsValid
          ? computePaymentAndValue(p.pricePerByte, sizeBytes, durationBlocks)
          : { payment: 0n, value: 0n }
        return (
          <div
            key={p.account}
            className="flex items-center justify-between gap-3 rounded-md border border-gray-800 bg-gray-900/40 p-3"
            data-testid="provider-row"
          >
            <div className="min-w-0 space-y-1">
              <div className="flex items-center gap-2">
                <HardDrive className="h-4 w-4 shrink-0 text-gray-500" />
                <code className="truncate text-sm text-gray-200" title={p.account}>
                  {formatAddress(p.account, 6)}
                </code>
                {eligible ? (
                  <Badge variant="secondary">
                    {p.maxCapacity === 0n ? 'unmetered' : formatBytes(p.availableCapacity)} free
                  </Badge>
                ) : (
                  <Badge variant="warning">{reasons[0]}</Badge>
                )}
              </div>
              {inputsValid && (
                <div className="text-xs text-gray-500">
                  ~{formatTokens(value)} tokens
                  <span className="text-gray-600">
                    {' '}
                    (agreement {formatTokens(payment)}, rest a refundable buffer)
                  </span>
                </div>
              )}
            </div>
            <Button
              size="sm"
              disabled={!eligible || !inputsValid}
              onClick={() => onCreate(p)}
              data-testid="create-library-button"
            >
              Create
            </Button>
          </div>
        )
      })}
    </div>
  )
}
