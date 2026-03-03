import { StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { Subscribe } from '@react-rxjs/core'
import App from './App'
import { connect as connectChain } from '@/state/chain.state'
import { restoreWalletConnection } from '@/state/wallet.state'
import './index.css'

function AppWithInit() {
  useEffect(() => {
    // Connect to chain and restore wallet on page load
    const init = async () => {
      try {
        // First connect to chain
        await connectChain()
      } catch (e) {
        console.warn('Failed to connect to chain:', e)
      }

      try {
        // Then try to restore saved wallet connection
        await restoreWalletConnection()
      } catch (e) {
        console.warn('Failed to restore wallet:', e)
      }
    }

    init()
  }, [])

  return <App />
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Subscribe>
      <BrowserRouter>
        <AppWithInit />
      </BrowserRouter>
    </Subscribe>
  </StrictMode>
)
