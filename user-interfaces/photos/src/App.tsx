// SPDX-License-Identifier: GPL-3.0-only

import { Header } from '@/components/Header'
import { Library } from '@/pages/Library'

function App() {
  return (
    <div className="min-h-screen bg-gray-950 text-gray-100">
      <Header />
      <main className="container mx-auto px-4 py-8">
        <Library />
      </main>
    </div>
  )
}

export default App
