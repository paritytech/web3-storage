import { Link, useLocation } from 'react-router-dom'
import { Server, FileText, Shield, Coins, CheckCircle, Settings } from 'lucide-react'
import { useConnectionStatus, useBlockNumber, connect as connectChain } from '@/state/chain.state'
import { useSelectedAccount, useWalletStatus, connectWallet } from '@/state/wallet.state'
import { Button } from '@/components/ui/Button'
import { Badge } from '@/components/ui/Badge'
import { Spinner } from '@/components/ui/Spinner'
import { formatAddress } from '@/utils/format'

const navItems = [
  { path: '/', label: 'Overview', icon: Server },
  { path: '/registration', label: 'Registration', icon: Settings },
  { path: '/agreements', label: 'Agreements', icon: FileText },
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

  const handleConnect = async () => {
    try {
      // Connect to chain first, then wallet
      if (connectionStatus !== 'connected') {
        await connectChain()
      }
      await connectWallet()
    } catch (error) {
      console.error('Connection failed:', error)
    }
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
                <Badge variant="secondary">#{blockNumber.toLocaleString()}</Badge>
              )}
            </div>

            {/* Account */}
            {selectedAccount ? (
              <Button variant="outline" size="sm">
                {formatAddress(selectedAccount.address)}
              </Button>
            ) : (
              <Button
                size="sm"
                onClick={handleConnect}
                disabled={walletStatus === 'connecting'}
              >
                {walletStatus === 'connecting' ? (
                  <>
                    <Spinner size="sm" className="mr-2" />
                    Connecting...
                  </>
                ) : (
                  'Connect Wallet'
                )}
              </Button>
            )}
          </div>
        </div>
      </div>
    </header>
  )
}
