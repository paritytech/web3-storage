import { useState } from 'react'
import { Link, useLocation } from 'react-router-dom'
import {
  Server,
  FileText,
  Database,
  Shield,
  Coins,
  CheckCircle,
  Settings,
  ChevronDown,
  TestTube,
  Wallet,
} from 'lucide-react'
import { useConnectionStatus, useBlockNumber, connect as connectChain } from '@/state/chain.state'
import {
  useSelectedAccount,
  useWalletStatus,
  useWalletMode,
  useAvailableExtensions,
  useAccounts,
  refreshExtensions,
  connectExtension,
  connectDevAccounts,
  selectAccount,
  disconnectWallet,
} from '@/state/wallet.state'
import { NetworkSelector } from '@/components/NetworkSelector'
import { Button } from '@/components/ui/Button'
import { Badge } from '@/components/ui/Badge'
import { Spinner } from '@/components/ui/Spinner'
import { formatAddress } from '@/utils/format'

const navItems = [
  { path: '/', label: 'Overview', icon: Server },
  { path: '/registration', label: 'Registration', icon: Settings },
  { path: '/agreements', label: 'Agreements', icon: FileText },
  { path: '/buckets', label: 'Buckets', icon: Database },
  { path: '/checkpoints', label: 'Checkpoints', icon: CheckCircle },
  { path: '/challenges', label: 'Challenges', icon: Shield },
  { path: '/earnings', label: 'Earnings', icon: Coins },
]

export function Header() {
  const location = useLocation()
  const connectionStatus = useConnectionStatus()
  const blockNumber = useBlockNumber()
  const selectedAccount = useSelectedAccount()
  const walletStatus = useWalletStatus()
  const walletMode = useWalletMode()
  const extensions = useAvailableExtensions()
  const accounts = useAccounts()

  const [showConnectOptions, setShowConnectOptions] = useState(false)
  const [showExtensionPicker, setShowExtensionPicker] = useState(false)
  const [showAccountPicker, setShowAccountPicker] = useState(false)

  const handleConnectClick = async () => {
    // Connect to chain first if needed
    if (connectionStatus !== 'connected') {
      try {
        await connectChain()
      } catch (error) {
        console.error('Failed to connect to chain:', error)
        return
      }
    }

    // Show connection options
    setShowConnectOptions(true)
  }

  const handleDevConnect = async () => {
    setShowConnectOptions(false)
    try {
      await connectDevAccounts()
    } catch (error) {
      console.error('Failed to connect dev accounts:', error)
    }
  }

  const handleExtensionConnectClick = () => {
    setShowConnectOptions(false)
    const available = refreshExtensions()

    if (available.length === 0) {
      alert('No wallet extensions found. Please install Polkadot.js, Talisman, or SubWallet.')
      return
    }

    if (available.length === 1) {
      handleExtensionSelect(available[0])
    } else {
      setShowExtensionPicker(true)
    }
  }

  const handleExtensionSelect = async (extensionName: string) => {
    setShowExtensionPicker(false)
    try {
      await connectExtension(extensionName)
    } catch (error) {
      console.error('Failed to connect to extension:', error)
    }
  }

  const handleAccountSelect = async (address: string) => {
    setShowAccountPicker(false)
    try {
      await selectAccount(address)
    } catch (error) {
      console.error('Failed to select account:', error)
    }
  }

  const handleDisconnect = () => {
    setShowAccountPicker(false)
    disconnectWallet()
  }

  const closeAllDropdowns = () => {
    setShowConnectOptions(false)
    setShowExtensionPicker(false)
    setShowAccountPicker(false)
  }

  return (
    <header className="border-b border-gray-800 bg-gray-900/50 backdrop-blur-sm sticky top-0 z-50">
      <div className="container mx-auto px-4">
        <div className="flex items-center justify-between h-16">
          {/* Logo */}
          <Link to="/" className="flex items-center gap-2">
            <Server className="h-6 w-6 text-purple-500" />
            <span className="font-bold text-lg">Provider Dashboard</span>
          </Link>

          {/* Navigation */}
          <nav className="hidden md:flex items-center gap-1">
            {navItems.map(({ path, label, icon: Icon }) => (
              <Link
                key={path}
                to={path}
                data-testid={`nav-${label.toLowerCase()}`}
                className={`flex items-center gap-2 px-3 py-2 rounded-md text-sm font-medium transition-colors ${
                  location.pathname === path
                    ? 'bg-purple-500/20 text-purple-400'
                    : 'text-gray-400 hover:text-gray-100 hover:bg-gray-800'
                }`}
              >
                <Icon className="h-4 w-4" />
                {label}
              </Link>
            ))}
          </nav>

          {/* Status & Account */}
          <div className="flex items-center gap-4">
            {/* Network Selector */}
            <NetworkSelector />

            {/* Connection Status */}
            <div className="flex items-center gap-2">
              <div
                className={`h-2 w-2 rounded-full ${
                  connectionStatus === 'connected'
                    ? 'bg-green-500'
                    : connectionStatus === 'connecting'
                    ? 'bg-yellow-500 animate-pulse'
                    : 'bg-red-500'
                }`}
              />
              {connectionStatus === 'connected' && blockNumber > 0 && (
                <Badge variant="secondary" data-testid="block-number">#{blockNumber.toLocaleString()}</Badge>
              )}
            </div>

            {/* Account */}
            {selectedAccount ? (
              <div className="relative">
                <Button
                  data-testid="provider-account-button"
                  variant="outline"
                  size="sm"
                  onClick={() => setShowAccountPicker(!showAccountPicker)}
                  className="flex items-center gap-2"
                >
                  {walletMode === 'dev' && (
                    <TestTube className="h-3 w-3 text-yellow-500" />
                  )}
                  <span data-testid="provider-account-name">{selectedAccount.name || formatAddress(selectedAccount.address)}</span>
                  <ChevronDown className="h-4 w-4" />
                </Button>

                {/* Account Picker Dropdown */}
                {showAccountPicker && (
                  <div className="absolute right-0 mt-2 w-72 bg-gray-800 border border-gray-700 rounded-lg shadow-xl overflow-hidden z-50">
                    {/* Mode indicator */}
                    <div className="px-3 py-2 bg-gray-900/50 border-b border-gray-700">
                      {walletMode === 'dev' ? (
                        <div className="flex items-center gap-2 text-yellow-500 text-xs">
                          <TestTube className="h-3 w-3" />
                          <span>Development Mode - Test accounts only</span>
                        </div>
                      ) : (
                        <div className="flex items-center gap-2 text-green-500 text-xs">
                          <Wallet className="h-3 w-3" />
                          <span>Connected via browser extension</span>
                        </div>
                      )}
                    </div>

                    <div className="p-2 border-b border-gray-700">
                      <div className="text-xs text-gray-400">Connected Account</div>
                      <div className="text-sm font-medium truncate">
                        {selectedAccount.name || 'Account'}
                      </div>
                      <div className="text-xs text-gray-500 truncate">
                        {selectedAccount.address}
                      </div>
                    </div>

                    {accounts.length > 1 && (
                      <div className="max-h-48 overflow-y-auto">
                        <div className="p-2 text-xs text-gray-400 border-b border-gray-700">
                          Switch Account
                        </div>
                        {accounts.map((account) => (
                          <button
                            key={account.address}
                            data-testid={`provider-account-select-${account.name || account.address}`}
                            onClick={() => handleAccountSelect(account.address)}
                            className={`w-full text-left px-3 py-2 text-sm hover:bg-gray-700 ${
                              account.address === selectedAccount.address
                                ? 'bg-gray-700/50'
                                : ''
                            }`}
                          >
                            <div className="font-medium truncate">
                              {account.name || 'Account'}
                            </div>
                            <div className="text-xs text-gray-500 truncate">
                              {formatAddress(account.address)}
                            </div>
                          </button>
                        ))}
                      </div>
                    )}

                    <button
                      data-testid="provider-disconnect"
                      onClick={handleDisconnect}
                      className="w-full text-left px-3 py-2 text-sm text-red-400 hover:bg-gray-700 border-t border-gray-700"
                    >
                      Disconnect
                    </button>
                  </div>
                )}
              </div>
            ) : (
              <div className="relative">
                <Button
                  data-testid="provider-connect-button"
                  size="sm"
                  onClick={handleConnectClick}
                  disabled={walletStatus === 'connecting'}
                >
                  {walletStatus === 'connecting' ? (
                    <>
                      <Spinner size="sm" className="mr-2" />
                      Connecting...
                    </>
                  ) : (
                    'Connect'
                  )}
                </Button>

                {/* Connection Options Dropdown */}
                {showConnectOptions && (
                  <div className="absolute right-0 mt-2 w-64 bg-gray-800 border border-gray-700 rounded-lg shadow-xl overflow-hidden z-50">
                    <div className="p-2 border-b border-gray-700">
                      <div className="text-xs text-gray-400">Choose Connection Type</div>
                    </div>

                    {/* Dev Accounts Option */}
                    <button
                      data-testid="provider-connect-dev"
                      onClick={handleDevConnect}
                      className="w-full text-left px-3 py-3 hover:bg-gray-700 border-b border-gray-700"
                    >
                      <div className="flex items-center gap-2">
                        <TestTube className="h-4 w-4 text-yellow-500" />
                        <span className="font-medium">Dev Accounts</span>
                        <Badge variant="warning" className="text-xs ml-auto">Local</Badge>
                      </div>
                      <p className="text-xs text-gray-500 mt-1 ml-6">
                        Use Alice, Bob, etc. for local development. No real funds.
                      </p>
                    </button>

                    {/* Browser Extension Option */}
                    <button
                      data-testid="provider-connect-wallet"
                      onClick={handleExtensionConnectClick}
                      className="w-full text-left px-3 py-3 hover:bg-gray-700"
                    >
                      <div className="flex items-center gap-2">
                        <Wallet className="h-4 w-4 text-purple-400" />
                        <span className="font-medium">Browser Wallet</span>
                      </div>
                      <p className="text-xs text-gray-500 mt-1 ml-6">
                        Connect Polkadot.js, Talisman, or SubWallet for testnet/mainnet.
                      </p>
                    </button>

                    <button
                      onClick={() => setShowConnectOptions(false)}
                      className="w-full text-left px-3 py-2 text-sm text-gray-400 hover:bg-gray-700 border-t border-gray-700"
                    >
                      Cancel
                    </button>
                  </div>
                )}

                {/* Extension Picker Dropdown */}
                {showExtensionPicker && extensions.length > 0 && (
                  <div className="absolute right-0 mt-2 w-56 bg-gray-800 border border-gray-700 rounded-lg shadow-xl overflow-hidden z-50">
                    <div className="p-2 border-b border-gray-700">
                      <div className="text-xs text-gray-400">Select Wallet Extension</div>
                    </div>
                    {extensions.map((ext) => (
                      <button
                        key={ext}
                        onClick={() => handleExtensionSelect(ext)}
                        className="w-full text-left px-3 py-2 text-sm hover:bg-gray-700 capitalize"
                      >
                        {ext}
                      </button>
                    ))}
                    <button
                      onClick={() => setShowExtensionPicker(false)}
                      className="w-full text-left px-3 py-2 text-sm text-gray-400 hover:bg-gray-700 border-t border-gray-700"
                    >
                      Cancel
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Click outside to close dropdowns */}
      {(showConnectOptions || showExtensionPicker || showAccountPicker) && (
        <div className="fixed inset-0 z-40" onClick={closeAllDropdowns} />
      )}
    </header>
  )
}
