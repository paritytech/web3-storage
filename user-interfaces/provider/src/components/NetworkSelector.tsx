import { useState } from 'react'
import { ChevronDown, Globe, Wifi } from 'lucide-react'
import {
  useSelectedNetwork,
  useNetworkList,
  selectNetwork,
  selectCustomNetwork,
} from '@/state/network.state'
import type { NetworkConfig, NetworkId } from '@web3-storage/network-config'

function networkDotColor(network: NetworkConfig): string {
  if (network.id === 'local') return 'bg-green-500'
  if (network.isTestnet) return 'bg-blue-500'
  return 'bg-purple-500'
}

export function NetworkSelector() {
  const selectedNetwork = useSelectedNetwork()
  const networkList = useNetworkList()

  const [showDropdown, setShowDropdown] = useState(false)
  const [showCustomForm, setShowCustomForm] = useState(false)
  const [customWs, setCustomWs] = useState('')
  const [customHttp, setCustomHttp] = useState('')

  const handleSelect = async (id: NetworkId) => {
    if (id === 'custom') {
      setShowCustomForm(true)
      return
    }
    setShowDropdown(false)
    try {
      await selectNetwork(id)
    } catch (error) {
      console.error('Failed to switch network:', error)
    }
  }

  const handleCustomSubmit = async () => {
    if (!customWs.trim()) return
    setShowDropdown(false)
    setShowCustomForm(false)
    try {
      await selectCustomNetwork({
        parachainWs: customWs.trim(),
        providerHttp: customHttp.trim(),
      })
    } catch (error) {
      console.error('Failed to connect to custom network:', error)
    }
  }

  const handleCustomCancel = () => {
    setShowCustomForm(false)
    setCustomWs('')
    setCustomHttp('')
  }

  return (
    <div className="relative">
      <button
        onClick={() => setShowDropdown(!showDropdown)}
        className="flex items-center gap-2 px-2 py-1 rounded-md text-sm text-gray-300 hover:bg-gray-800 transition-colors"
      >
        <div className={`h-2 w-2 rounded-full ${networkDotColor(selectedNetwork)}`} />
        <span className="hidden sm:inline">{selectedNetwork.name}</span>
        <ChevronDown className="h-3 w-3" />
      </button>

      {showDropdown && (
        <>
          {/* Backdrop */}
          <div className="fixed inset-0 z-40" onClick={() => { setShowDropdown(false); handleCustomCancel() }} />

          {/* Dropdown */}
          <div className="absolute right-0 mt-2 w-72 bg-gray-800 border border-gray-700 rounded-lg shadow-xl overflow-hidden z-50">
            <div className="px-3 py-2 border-b border-gray-700">
              <div className="flex items-center gap-2 text-xs text-gray-400">
                <Globe className="h-3 w-3" />
                <span>Select Network</span>
              </div>
            </div>

            {/* Network list */}
            {networkList.map((network) => (
              <button
                key={network.id}
                onClick={() => handleSelect(network.id)}
                className={`w-full text-left px-3 py-2.5 text-sm hover:bg-gray-700 flex items-center gap-3 ${
                  network.id === selectedNetwork.id ? 'bg-gray-700/50' : ''
                }`}
              >
                <div className={`h-2 w-2 rounded-full flex-shrink-0 ${networkDotColor(network)}`} />
                <div className="flex-1 min-w-0">
                  <div className="font-medium">{network.name}</div>
                  <div className="text-xs text-gray-500 truncate">{network.parachainWs}</div>
                </div>
                {network.isTestnet && (
                  <span className="text-xs text-blue-400 bg-blue-400/10 px-1.5 py-0.5 rounded flex-shrink-0">
                    testnet
                  </span>
                )}
              </button>
            ))}

            {/* Custom RPC option */}
            {!showCustomForm ? (
              <button
                onClick={() => handleSelect('custom')}
                className={`w-full text-left px-3 py-2.5 text-sm hover:bg-gray-700 flex items-center gap-3 border-t border-gray-700 ${
                  selectedNetwork.id === 'custom' ? 'bg-gray-700/50' : ''
                }`}
              >
                <Wifi className="h-3 w-3 text-gray-400 flex-shrink-0" />
                <div>
                  <div className="font-medium">Custom RPC</div>
                  <div className="text-xs text-gray-500">Connect to a custom endpoint</div>
                </div>
              </button>
            ) : (
              <div className="border-t border-gray-700 p-3 space-y-2">
                <div className="text-xs text-gray-400 font-medium">Custom RPC Endpoints</div>
                <input
                  type="text"
                  placeholder="Parachain WS (e.g. ws://127.0.0.1:9944)"
                  value={customWs}
                  onChange={(e) => setCustomWs(e.target.value)}
                  className="w-full px-2 py-1.5 text-xs bg-gray-900 border border-gray-600 rounded text-gray-200 placeholder-gray-500 focus:outline-none focus:border-purple-500"
                />
                <input
                  type="text"
                  placeholder="Provider HTTP (e.g. http://localhost:3333)"
                  value={customHttp}
                  onChange={(e) => setCustomHttp(e.target.value)}
                  className="w-full px-2 py-1.5 text-xs bg-gray-900 border border-gray-600 rounded text-gray-200 placeholder-gray-500 focus:outline-none focus:border-purple-500"
                />
                <div className="flex gap-2">
                  <button
                    onClick={handleCustomSubmit}
                    disabled={!customWs.trim()}
                    className="flex-1 px-2 py-1 text-xs bg-purple-600 hover:bg-purple-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-white"
                  >
                    Connect
                  </button>
                  <button
                    onClick={handleCustomCancel}
                    className="px-2 py-1 text-xs text-gray-400 hover:text-gray-200"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}
          </div>
        </>
      )}
    </div>
  )
}
