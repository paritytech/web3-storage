// SPDX-License-Identifier: GPL-3.0-only
//
// The profile menu: the host account and its balance, the bucket account's
// balance, how much of the plan the library uses, and the way to change the
// plan. The host owns the account, so there is no sign-in or account switch
// here.

import { useRef, useState } from 'react'
import { Check, Copy, CreditCard, User } from 'lucide-react'
import { useBucketFreeBalance, useFreeBalance, useSelectedAccount, useWalletStatus } from '@/state/wallet.state'
import { hasLibrary, usePlan } from '@/state/library.state'
import { openPlanPanel } from '@/state/ui.state'
import { StorageUsage } from '@/components/StorageUsage'
import { formatAddress, formatTokens } from '@web3-storage/format'
import { useClickOutside } from '@/lib/use-click-outside'

export function ProfileMenu() {
  const account = useSelectedAccount()
  const status = useWalletStatus()
  const balance = useFreeBalance()
  const bucketBalance = useBucketFreeBalance()
  const plan = usePlan()
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  useClickOutside(containerRef, open, () => setOpen(false))

  async function copyAddress() {
    if (!account) return
    await navigator.clipboard.writeText(account.address)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div ref={containerRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-label="Profile"
        className="flex h-9 w-9 cursor-pointer items-center justify-center rounded-full bg-action-secondary text-primary transition-colors hover:bg-action-secondary-hover"
        data-testid="profile-button"
      >
        <User className="h-5 w-5" />
      </button>

      {open && (
        <div
          className="absolute right-0 top-full z-50 mt-2 w-72 rounded-container bg-surface-container p-4 shadow-2"
          data-testid="profile-menu"
        >
          <p className="text-xs text-tertiary">Account from your host</p>
          {account ? (
            <>
              <button
                type="button"
                onClick={() => void copyAddress()}
                title={account.address}
                className="mt-1 flex w-full cursor-pointer items-center justify-between gap-2 rounded-small px-2 py-1.5 -mx-2 text-sm font-medium text-primary transition-colors hover:bg-selection-container-hover"
                data-testid="profile-address"
              >
                <span className="font-mono">{formatAddress(account.address, 6)}</span>
                {copied ? <Check className="h-4 w-4 text-success" /> : <Copy className="h-4 w-4 text-secondary" />}
              </button>
              <div className="mt-3 flex items-baseline justify-between" data-testid="profile-balance">
                <span className="text-xs text-tertiary">Balance</span>
                <span className="text-sm font-medium tabular-nums text-primary">
                  {balance === undefined ? '—' : formatTokens(balance)}
                </span>
              </div>
              <div className="mt-2 flex items-baseline justify-between" data-testid="profile-bucket-balance">
                <span className="text-xs text-tertiary">Bucket account balance</span>
                <span className="text-sm font-medium tabular-nums text-primary">
                  {bucketBalance === undefined ? '—' : formatTokens(bucketBalance)}
                </span>
              </div>
            </>
          ) : (
            <p className="mt-1 text-sm text-secondary">
              {status === 'error' ? 'Your host did not share an account.' : 'Reading account…'}
            </p>
          )}

          {plan && hasLibrary(plan) && (
            <>
              <div className="mt-4">
                <StorageUsage plan={plan} />
              </div>
              <button
                type="button"
                onClick={() => {
                  setOpen(false)
                  openPlanPanel()
                }}
                className="mt-3 -mx-2 flex w-[calc(100%+1rem)] cursor-pointer items-center gap-3 rounded-small px-2 py-2 text-sm font-medium text-primary transition-colors hover:bg-selection-container-hover"
                data-testid="profile-change-plan"
              >
                <CreditCard className="h-4 w-4 text-secondary" /> Change plan
              </button>
            </>
          )}
        </div>
      )}
    </div>
  )
}
