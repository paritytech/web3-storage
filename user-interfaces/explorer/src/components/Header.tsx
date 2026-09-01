// SPDX-License-Identifier: GPL-3.0-only

import { useState } from 'react'
import { Link, useLocation } from 'react-router-dom'
import {
  ArrowLeft,
  BarChart3,
  ChevronDown,
  Database,
  FileText,
  RefreshCw,
  Server,
  Shield,
  UserCheck,
} from 'lucide-react'
import { withNetworkHandoff } from '@web3-storage/network-config'
import { NetworkPicker } from '@web3-storage/network-picker'
import { Badge, Button } from '@/components/ui'
import { useBlockNumber, useConnectionStatus } from '@/state/chain.state'
import {
  useSelectedNetwork,
  useNetworkList,
  selectNetwork,
  selectCustomNetwork,
} from '@/state/network.state'
import {
  useAutoRefreshSecs,
  setAutoRefreshSecs,
  AUTO_REFRESH_OPTIONS,
  formatInterval,
} from '@/state/settings.state'
import {
  connectDevAccounts,
  connectExtension,
  disconnectWallet,
  refreshExtensions,
  useAvailableExtensions,
  useMyAddresses,
  useWalletMode,
} from '@/state/wallet.state'
import { loadAll } from '@/state/explorer.state'

const LANDING_URL = import.meta.env.DEV ? 'http://127.0.0.1:5176/' : '../'

const navItems = [
  { path: '/', label: 'Summary', icon: BarChart3 },
  { path: '/providers', label: 'Providers', icon: Server },
  { path: '/agreements', label: 'Agreements', icon: FileText },
  { path: '/buckets', label: 'Buckets', icon: Database },
  { path: '/challenges', label: 'Challenges', icon: Shield },
]

const statusColors = {
  connected: 'bg-green-500',
  connecting: 'bg-yellow-500 animate-pulse',
  disconnected: 'bg-gray-500',
  error: 'bg-red-500',
}

type OpenMenu = 'none' | 'refresh' | 'wallet'

export function Header() {
  const location = useLocation()
  const connectionStatus = useConnectionStatus()
  const blockNumber = useBlockNumber()
  const selectedNetwork = useSelectedNetwork()
  const networkList = useNetworkList()
  const autoRefreshSecs = useAutoRefreshSecs()
  const walletMode = useWalletMode()
  const myAddresses = useMyAddresses()
  const [openMenu, setOpenMenu] = useState<OpenMenu>('none')

  const toggleMenu = (menu: OpenMenu) => setOpenMenu((cur) => (cur === menu ? 'none' : menu))

  return (
    <header className="border-b border-gray-800 bg-gray-900/50">
      <div className="mx-auto flex max-w-7xl flex-wrap items-center gap-x-6 gap-y-2 px-4 py-3">
        <div className="flex items-center gap-3">
          <a
            href={withNetworkHandoff(LANDING_URL, selectedNetwork)}
            className="text-gray-400 hover:text-gray-200"
            title="Back to landing page"
            data-testid="back-to-landing"
          >
            <ArrowLeft className="h-4 w-4" />
          </a>
          <Link to="/" className="text-lg font-semibold text-gray-100">
            Web3 Storage Dashboard
          </Link>
        </div>

        <nav className="flex items-center gap-1">
          {navItems.map(({ path, label, icon: Icon }) => {
            const active =
              path === '/' ? location.pathname === '/' : location.pathname.startsWith(path)
            return (
              <Link
                key={path}
                to={path}
                data-testid={`nav-${label.toLowerCase()}`}
                className={`flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm ${
                  active
                    ? 'bg-purple-500/20 text-purple-300'
                    : 'text-gray-400 hover:bg-gray-800 hover:text-gray-200'
                }`}
              >
                <Icon className="h-4 w-4" />
                {label}
              </Link>
            )
          })}
        </nav>

        <div className="ml-auto flex items-center gap-3">
          <div className="flex items-center gap-2">
            <span
              className={`h-2 w-2 rounded-full ${statusColors[connectionStatus]}`}
              title={connectionStatus}
              data-testid="connection-status"
            />
            {blockNumber > 0 && (
              <Badge variant="secondary" data-testid="block-number">
                #{blockNumber.toLocaleString()}
              </Badge>
            )}
          </div>

          <div className="relative">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => toggleMenu('refresh')}
              data-testid="refresh-settings-button"
            >
              <RefreshCw className="mr-1.5 h-3.5 w-3.5" />
              <span className="text-xs" data-testid="auto-refresh-current">
                {formatInterval(autoRefreshSecs)}
              </span>
              <ChevronDown className="ml-1 h-3 w-3" />
            </Button>
            {openMenu === 'refresh' && (
              <>
                <div className="fixed inset-0 z-10" onClick={() => setOpenMenu('none')} />
                <div className="absolute right-0 z-20 mt-1 w-44 rounded-md border border-gray-700 bg-gray-900 p-1 shadow-lg">
                  <p className="px-2 py-1 text-xs text-gray-500">Auto-refresh</p>
                  {AUTO_REFRESH_OPTIONS.map((opt) => (
                    <button
                      key={opt}
                      className={`block w-full rounded px-2 py-1 text-left text-sm hover:bg-gray-800 ${
                        opt === autoRefreshSecs ? 'text-purple-300' : 'text-gray-300'
                      }`}
                      onClick={() => {
                        setAutoRefreshSecs(opt)
                        setOpenMenu('none')
                      }}
                      data-testid={`auto-refresh-${opt}`}
                    >
                      {formatInterval(opt)}
                    </button>
                  ))}
                  <div className="my-1 border-t border-gray-800" />
                  <button
                    className="block w-full rounded px-2 py-1 text-left text-sm text-gray-300 hover:bg-gray-800"
                    onClick={() => {
                      void loadAll({ silent: true })
                      setOpenMenu('none')
                    }}
                    data-testid="refresh-now"
                  >
                    Refresh now
                  </button>
                </div>
              </>
            )}
          </div>

          <div className="relative">
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                refreshExtensions()
                toggleMenu('wallet')
              }}
              data-testid="highlight-mine-button"
            >
              <UserCheck className="mr-1.5 h-3.5 w-3.5" />
              <span className="text-xs">
                {walletMode === 'none'
                  ? 'Highlight mine'
                  : `Mine: ${myAddresses.length} account${myAddresses.length === 1 ? '' : 's'}`}
              </span>
              <ChevronDown className="ml-1 h-3 w-3" />
            </Button>
            {openMenu === 'wallet' && (
              <WalletMenu onClose={() => setOpenMenu('none')} />
            )}
          </div>

          <NetworkPicker
            compact
            theme="dark"
            selectedNetwork={selectedNetwork}
            networkList={networkList}
            onSelect={(id) => {
              // connect() re-throws after publishing connectionError$; the
              // banner already reports it, so swallow the rejection here.
              selectNetwork(id).catch(() => {})
            }}
            onSelectCustom={(input) => {
              selectCustomNetwork(input).catch(() => {})
            }}
          />
        </div>
      </div>
    </header>
  )
}

function WalletMenu({ onClose }: { onClose: () => void }) {
  const walletMode = useWalletMode()
  const extensions = useAvailableExtensions()

  return (
    <>
      <div className="fixed inset-0 z-10" onClick={onClose} />
      <div className="absolute right-0 z-20 mt-1 w-56 rounded-md border border-gray-700 bg-gray-900 p-1 shadow-lg">
        <p className="px-2 py-1 text-xs text-gray-500">
          Highlight rows involving your accounts. Read-only — nothing is ever signed.
        </p>
        <button
          className="block w-full rounded px-2 py-1 text-left text-sm text-gray-300 hover:bg-gray-800"
          onClick={() => {
            connectDevAccounts()
            onClose()
          }}
          data-testid="wallet-dev-accounts"
        >
          Dev accounts (Alice…Ferdie)
        </button>
        {extensions.map((name) => (
          <button
            key={name}
            className="block w-full rounded px-2 py-1 text-left text-sm text-gray-300 hover:bg-gray-800"
            onClick={() => {
              void connectExtension(name).catch(() => {})
              onClose()
            }}
            data-testid={`wallet-extension-${name}`}
          >
            {name}
          </button>
        ))}
        {extensions.length === 0 && (
          <p className="px-2 py-1 text-xs text-gray-600">No wallet extensions detected</p>
        )}
        {walletMode !== 'none' && (
          <>
            <div className="my-1 border-t border-gray-800" />
            <button
              className="block w-full rounded px-2 py-1 text-left text-sm text-gray-300 hover:bg-gray-800"
              onClick={() => {
                disconnectWallet()
                onClose()
              }}
              data-testid="wallet-disconnect"
            >
              Stop highlighting
            </button>
          </>
        )}
      </div>
    </>
  )
}
