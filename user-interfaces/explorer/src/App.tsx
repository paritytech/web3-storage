// SPDX-License-Identifier: GPL-3.0-only

import { useEffect } from 'react'
import { Routes, Route } from 'react-router-dom'
import { Header } from '@/components/Header'
import { Summary, Providers, Agreements, Buckets, Challenges } from '@/pages'
import { useConnectionError, useIsConnected } from '@/state/chain.state'
import { useParachainWs } from '@/state/network.state'
import { useAutoRefreshSecs } from '@/state/settings.state'
import { loadAll, useExplorerError } from '@/state/explorer.state'

function App() {
  const connected = useIsConnected()
  const connectionError = useConnectionError()
  const explorerError = useExplorerError()
  const parachainWs = useParachainWs()
  const autoRefreshSecs = useAutoRefreshSecs()

  // Full reload on connect — keyed on the endpoint too, so a network switch
  // reloads instead of showing the previous network's data.
  useEffect(() => {
    if (connected) void loadAll()
  }, [connected, parachainWs])

  useEffect(() => {
    if (!connected || autoRefreshSecs <= 0) return
    const id = setInterval(() => {
      void loadAll({ silent: true })
    }, autoRefreshSecs * 1000)
    return () => clearInterval(id)
  }, [connected, autoRefreshSecs, parachainWs])

  return (
    <div className="min-h-screen bg-gray-950 text-gray-100">
      <Header />
      {connectionError && (
        <div
          className="mx-auto mt-4 max-w-7xl rounded-md border border-red-900 bg-red-950/50 p-3 text-sm text-red-300"
          data-testid="connection-error"
        >
          Could not connect to the chain: {connectionError}
        </div>
      )}
      {explorerError && (
        <div
          className="mx-auto mt-4 max-w-7xl rounded-md border border-red-900 bg-red-950/50 p-3 text-sm text-red-300"
          data-testid="snapshot-error"
        >
          Failed to load network state: {explorerError}
        </div>
      )}
      <main className="mx-auto max-w-7xl px-4 py-6">
        <Routes>
          <Route path="/" element={<Summary />} />
          <Route path="/providers" element={<Providers />} />
          <Route path="/agreements" element={<Agreements />} />
          <Route path="/buckets" element={<Buckets />} />
          <Route path="/challenges" element={<Challenges />} />
        </Routes>
      </main>
    </div>
  )
}

export default App
