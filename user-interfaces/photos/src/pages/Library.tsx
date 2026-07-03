// SPDX-License-Identifier: GPL-3.0-only
//
// M4 — read-only state detection. Reads the Photos contract's `libraryOf` for
// the selected account (unsigned) and renders State A ("no library") vs State B
// ("drive #N"). No writes: creating a library is M5, albums/upload/grid are M6.

import { useEffect, useState } from 'react'
import { Image, FolderPlus, Anchor, AlertTriangle, Settings2 } from 'lucide-react'
import { useSelectedAccount } from '@/state/wallet.state'
import { useSelectedNetwork } from '@/state/network.state'
import { useConnectionStatus, useConnectionError } from '@/state/chain.state'
import { requireApi } from '@/lib/chain-client'
import {
  readLibraryOf,
  resolveContractAddress,
  setContractOverride,
  substrateToH160,
  isZeroRoot,
  type LibraryState,
  type ResolvedContract,
} from '@/lib/photos-contract'
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import { Button } from '@/components/ui/Button'
import { Spinner } from '@/components/ui/Spinner'
import { formatAddress, formatHash } from '@/utils/format'

type ReadState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ready'; library: LibraryState; contract: ResolvedContract }
  | { kind: 'error'; message: string }

export function Library() {
  const account = useSelectedAccount()
  const network = useSelectedNetwork()
  const connectionStatus = useConnectionStatus()
  const connectionError = useConnectionError()
  const [state, setState] = useState<ReadState>({ kind: 'idle' })
  const [refresh, setRefresh] = useState(0)

  const contract = resolveContractAddress(network)

  useEffect(() => {
    if (!account) {
      setState({ kind: 'idle' })
      return
    }
    if (connectionStatus !== 'connected') {
      setState({ kind: 'loading' })
      return
    }
    if (!contract) {
      setState({ kind: 'idle' })
      return
    }

    let cancelled = false
    setState({ kind: 'loading' })
    ;(async () => {
      try {
        const userH160 = substrateToH160(account.polkadotSigner.publicKey)
        const library = await readLibraryOf(requireApi(), contract.address, userH160, account.address)
        if (!cancelled) setState({ kind: 'ready', library, contract })
      } catch (e) {
        if (!cancelled) {
          setState({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
        }
      }
    })()
    return () => {
      cancelled = true
    }
    // `contract` is re-derived each render from network + storage; key the
    // effect on its address (not the object identity) plus the inputs that
    // change what we read.
  }, [account?.address, connectionStatus, contract?.address, network.id, refresh])

  // ── Not connected to a wallet yet ──
  if (!account) {
    return (
      <Centered>
        <Card className="max-w-md">
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Image className="h-5 w-5 text-purple-500" /> Welcome to Photos
            </CardTitle>
            <CardDescription>
              Your decentralized photo library, backed by Web3 Storage.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <p className="text-sm text-gray-400">
              Connect a wallet (top right) to view your library.
            </p>
          </CardContent>
        </Card>
      </Centered>
    )
  }

  // ── Contract address not configured ──
  if (!contract) {
    return (
      <Centered>
        <ContractConfig network={network.name} onSet={() => setRefresh((n) => n + 1)} unset />
      </Centered>
    )
  }

  // ── Connecting / reading ──
  if (state.kind === 'loading' || state.kind === 'idle') {
    return (
      <Centered>
        <div className="flex items-center gap-3 text-gray-400">
          <Spinner /> {connectionStatus === 'connected' ? 'Reading your library…' : 'Connecting to chain…'}
        </div>
      </Centered>
    )
  }

  // ── Read failed ──
  if (state.kind === 'error') {
    return (
      <Centered>
        <Card className="max-w-lg border-red-900/50">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-red-400">
              <AlertTriangle className="h-5 w-5" /> Could not read library
            </CardTitle>
            <CardDescription>
              {connectionError ? `Chain: ${connectionError}` : 'The contract read reverted or the chain was unreachable.'}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <pre className="text-xs text-gray-500 whitespace-pre-wrap break-words">{state.message}</pre>
            <div className="flex items-center gap-2">
              <Button size="sm" variant="outline" onClick={() => setRefresh((n) => n + 1)}>
                Retry
              </Button>
              <span className="text-xs text-gray-600">
                Contract {formatAddress(contract.address, 6)} ({contract.source})
              </span>
            </div>
          </CardContent>
        </Card>
      </Centered>
    )
  }

  const { library } = state
  const anchored = !isZeroRoot(library.rootCid)

  // ── State A — no library ──
  if (!library.exists) {
    return (
      <Centered>
        <Card className="max-w-md">
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <FolderPlus className="h-5 w-5 text-purple-500" /> No library yet
            </CardTitle>
            <CardDescription>
              {account.name || formatAddress(account.address)} hasn't set up a photo library on{' '}
              {network.name}.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <p className="text-sm text-gray-400">
              Creating a library — picking a storage provider and opening an agreement — lands in
              the next milestone (M5). This skeleton is read-only.
            </p>
            <Button disabled title="Coming in M5">
              Create library
            </Button>
            <ContractFootnote contract={contract} />
          </CardContent>
        </Card>
      </Centered>
    )
  }

  // ── State B — has a library ──
  return (
    <div className="max-w-3xl mx-auto space-y-6">
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle className="flex items-center gap-2">
              <Image className="h-5 w-5 text-purple-500" /> Drive #{library.driveId.toString()}
            </CardTitle>
            <Badge variant="success">Library active</Badge>
          </div>
          <CardDescription>
            Owned for {account.name || formatAddress(account.address)} on {network.name}.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex items-start gap-2 text-sm">
            <Anchor className="h-4 w-4 mt-0.5 text-gray-500" />
            <div>
              <div className="text-gray-400">On-chain metadata anchor</div>
              {anchored ? (
                <code className="text-gray-200" data-testid="root-cid" title={library.rootCid}>
                  {formatHash(library.rootCid, 10, 8)}
                </code>
              ) : (
                <span className="text-gray-500" data-testid="root-cid">not yet anchored</span>
              )}
            </div>
          </div>

          <div className="rounded-md border border-gray-800 bg-gray-900/40 p-4">
            <div className="text-sm font-medium text-gray-300">Albums &amp; photos</div>
            <p className="text-xs text-gray-500 mt-1">
              Browsing albums, uploading, and the photo grid arrive in M6.
            </p>
          </div>

          <ContractFootnote contract={contract} />
        </CardContent>
      </Card>
    </div>
  )
}

function Centered({ children }: { children: React.ReactNode }) {
  return <div className="flex justify-center pt-16">{children}</div>
}

function ContractFootnote({ contract }: { contract: ResolvedContract }) {
  return (
    <p className="text-[11px] text-gray-600">
      Photos contract <code>{formatAddress(contract.address, 6)}</code> ({contract.source})
    </p>
  )
}

/**
 * Dev affordance: when no contract address is configured for the network, let
 * the operator paste the H160 that `just photos deploy` / `just photos flow`
 * prints. Persists to localStorage only — no chain write.
 */
function ContractConfig({
  network,
  onSet,
  unset,
}: {
  network: string
  onSet: () => void
  unset?: boolean
}) {
  const [value, setValue] = useState('')
  const valid = /^0x[0-9a-fA-F]{40}$/.test(value.trim())

  return (
    <Card className="max-w-lg">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Settings2 className="h-5 w-5 text-purple-500" /> Photos contract not configured
        </CardTitle>
        <CardDescription>
          {unset
            ? `No Photos contract address is set for ${network}.`
            : 'Set the deployed Photos contract address.'}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-sm text-gray-400">
          Deploy once with <code className="text-gray-300">just photos deploy</code> (or run{' '}
          <code className="text-gray-300">just photos flow</code>) and paste the printed{' '}
          <code className="text-gray-300">0x…</code> address. It's saved to this browser only.
        </p>
        <div className="flex items-center gap-2">
          <input
            data-testid="contract-address-input"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="0x0000000000000000000000000000000000000000"
            className="flex-1 rounded-md border border-gray-700 bg-gray-900 px-3 py-2 text-sm font-mono text-gray-200 placeholder:text-gray-600 focus:outline-none focus:ring-1 focus:ring-purple-500"
          />
          <Button
            size="sm"
            disabled={!valid}
            data-testid="contract-address-save"
            onClick={() => {
              setContractOverride(value.trim())
              onSet()
            }}
          >
            Use
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
