// SPDX-License-Identifier: GPL-3.0-only

import { StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { Subscribe } from '@react-rxjs/core'
import App from './App'
import { connect as connectChain } from '@/state/chain.state'
import { connectHostAccount } from '@/state/wallet.state'
import './index.css'

function AppWithInit() {
  useEffect(() => {
    // Both do nothing outside a host.
    connectChain().catch((e) => console.warn('Failed to connect to chain:', e))
    connectHostAccount().catch((e) => console.warn('Failed to read the host account:', e))
  }, [])

  return <App />
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Subscribe fallback={<div className="min-h-screen bg-surface-main" />}>
      <AppWithInit />
    </Subscribe>
  </StrictMode>
)
