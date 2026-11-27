// SPDX-License-Identifier: GPL-3.0-only

import { Header } from '@/components/Header'
import { Library } from '@/pages/Library'

function App() {
  return (
    <div className="min-h-screen bg-surface-main text-primary">
      <Header />
      <Library />
    </div>
  )
}

export default App
