import { useEffect } from 'react'
import { Routes, Route } from 'react-router-dom'
import { Header } from '@/components/Header'
import { Overview } from '@/pages/Overview'
import { Registration } from '@/pages/Registration'
import { Agreements } from '@/pages/Agreements'
import { Buckets } from '@/pages/Buckets'
import { Checkpoints } from '@/pages/Checkpoints'
import { Challenges } from '@/pages/Challenges'
import { Earnings } from '@/pages/Earnings'
import { useSelectedAccount } from '@/state/wallet.state'
import { loadProviderData, addChallengeFromEvent } from '@/state/provider.state'
import { subscribeToChallengeEvents } from '@/lib/chain-client'

function App() {
  const selectedAccount = useSelectedAccount()

  // Load provider data at app level so all pages have it on initial render
  useEffect(() => {
    if (selectedAccount?.address) {
      loadProviderData(selectedAccount.address)
    }
  }, [selectedAccount?.address])

  // Subscribe to challenge events in real-time so they're captured
  // while the block is still pinned (scanning old blocks doesn't work)
  useEffect(() => {
    if (!selectedAccount?.address) return
    const unsub = subscribeToChallengeEvents(selectedAccount.address, (challenge) => {
      addChallengeFromEvent(challenge)
    })
    return unsub
  }, [selectedAccount?.address])

  return (
    <div className="min-h-screen bg-gray-950 text-gray-100">
      <Header />
      <main className="container mx-auto px-4 py-8">
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/registration" element={<Registration />} />
          <Route path="/agreements" element={<Agreements />} />
          <Route path="/buckets" element={<Buckets />} />
          <Route path="/checkpoints" element={<Checkpoints />} />
          <Route path="/challenges" element={<Challenges />} />
          <Route path="/earnings" element={<Earnings />} />
        </Routes>
      </main>
    </div>
  )
}

export default App
