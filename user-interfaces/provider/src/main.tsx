import { StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { Subscribe } from '@react-rxjs/core'
import App from './App'
import { connect as connectChain } from '@/state/chain.state'
import { getParachainWs } from '@/state/network.state'
import { restoreWalletConnection } from '@/state/wallet.state'
import './index.css'

function AppWithInit() {
  useEffect(() => {
    // Connect to chain and restore wallet in parallel (non-blocking)
    connectChain(getParachainWs()).catch((e) => console.warn('Failed to connect to chain:', e))
    restoreWalletConnection().catch((e) => console.warn('Failed to restore wallet:', e))
  }, [])

  return <App />
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Subscribe fallback={<div className="min-h-screen bg-gray-950" />}>
      <BrowserRouter basename={import.meta.env.BASE_URL.replace(/\/$/, '') || '/'}>
        <AppWithInit />
      </BrowserRouter>
    </Subscribe>
  </StrictMode>
)
