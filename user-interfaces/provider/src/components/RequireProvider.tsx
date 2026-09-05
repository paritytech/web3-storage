// SPDX-License-Identifier: GPL-3.0-only

import { type ReactNode } from 'react'
import { type LucideIcon, Loader2 } from 'lucide-react'
import { useSelectedAccount } from '@/state/wallet.state'
import { useIsRegistered, useIsProviderLoading } from '@/state/provider.state'

interface RequireProviderProps {
  icon: LucideIcon
  pageName: string
  children: ReactNode
}

export function RequireProvider({ icon: Icon, pageName, children }: RequireProviderProps) {
  const selectedAccount = useSelectedAccount()
  const isRegistered = useIsRegistered()
  const isLoading = useIsProviderLoading()

  if (!selectedAccount) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Icon className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Connect Your Wallet</h2>
        <p className="text-gray-500">Connect a wallet to view {pageName}</p>
      </div>
    )
  }

  // Show loading spinner while provider data is being fetched,
  // instead of flashing "Not Registered" on page reload.
  if (!isRegistered && isLoading) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Loader2 className="h-8 w-8 text-gray-400 animate-spin mb-4" />
        <p className="text-gray-500">Loading provider data...</p>
      </div>
    )
  }

  if (!isRegistered) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <Icon className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Not Registered</h2>
        <p className="text-gray-500">Register as a provider to view {pageName}</p>
      </div>
    )
  }

  return <>{children}</>
}
