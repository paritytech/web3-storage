import { Routes, Route } from 'react-router-dom'
import { Header } from '@/components/Header'
import { Overview } from '@/pages/Overview'
import { Registration } from '@/pages/Registration'
import { Agreements } from '@/pages/Agreements'
import { Checkpoints } from '@/pages/Checkpoints'
import { Challenges } from '@/pages/Challenges'
import { Earnings } from '@/pages/Earnings'

function App() {
  return (
    <div className="min-h-screen bg-gray-950 text-gray-100">
      <Header />
      <main className="container mx-auto px-4 py-8">
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/registration" element={<Registration />} />
          <Route path="/agreements" element={<Agreements />} />
          <Route path="/checkpoints" element={<Checkpoints />} />
          <Route path="/challenges" element={<Challenges />} />
          <Route path="/earnings" element={<Earnings />} />
        </Routes>
      </main>
    </div>
  )
}

export default App
