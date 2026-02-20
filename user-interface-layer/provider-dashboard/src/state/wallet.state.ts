/**
 * Wallet State - Account management and balance tracking
 *
 * This state manages wallet connections (browser extensions like
 * Polkadot.js, Talisman, SubWallet) and account selection.
 *
 * New providers connect their wallet first, then we check if they're
 * registered on-chain to determine the appropriate UI flow.
 */

import { BehaviorSubject, combineLatest, map } from 'rxjs'
import { bind } from '@react-rxjs/core'
import { getAccountBalance, isProviderRegistered } from '@/lib/chain-client'

// Types
export interface Account {
  address: string
  name: string
  type: 'sr25519' | 'ed25519' | 'ecdsa'
  source: 'polkadot-js' | 'subwallet' | 'talisman' | 'local'
}

export interface AccountBalance {
  free: bigint
  reserved: bigint
  frozen: bigint
}

export type WalletStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

// State subjects
const walletStatus$ = new BehaviorSubject<WalletStatus>('disconnected')
const accounts$ = new BehaviorSubject<Account[]>([])
const selectedAddress$ = new BehaviorSubject<string | null>(null)
const balances$ = new BehaviorSubject<Map<string, AccountBalance>>(new Map())
const walletError$ = new BehaviorSubject<string | undefined>(undefined)

// Provider registration status (checked after wallet connect)
const registrationStatus$ = new BehaviorSubject<Map<string, boolean>>(new Map())

// Derived state
const selectedAccount$ = combineLatest([accounts$, selectedAddress$]).pipe(
  map(([accounts, address]) => accounts.find((a) => a.address === address) || null)
)

const selectedBalance$ = combineLatest([balances$, selectedAddress$]).pipe(
  map(([balances, address]) => (address ? balances.get(address) : null) || null)
)

const isSelectedRegistered$ = combineLatest([registrationStatus$, selectedAddress$]).pipe(
  map(([status, address]) => (address ? status.get(address) ?? null : null))
)

// React hooks
export const [useWalletStatus] = bind(walletStatus$, 'disconnected')
export const [useAccounts] = bind(accounts$, [])
export const [useSelectedAddress] = bind(selectedAddress$, null)
export const [useSelectedAccount] = bind(selectedAccount$, null)
export const [useSelectedBalance] = bind(selectedBalance$, null)
export const [useWalletError] = bind(walletError$, undefined)

// This returns: true (registered), false (not registered), null (checking/unknown)
export const [useIsSelectedRegistered] = bind(isSelectedRegistered$, null)

export const [useAccountBalance] = bind(
  (address: string) => balances$.pipe(map((balances) => balances.get(address) || null)),
  null
)

export const [useIsRegistered] = bind(
  (address: string) =>
    registrationStatus$.pipe(map((status) => status.get(address) ?? null)),
  null
)

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Connect to wallet extension
 *
 * This will prompt the user to authorize the app in their wallet extension.
 */
export async function connectWallet(): Promise<void> {
  walletStatus$.next('connecting')
  walletError$.next(undefined)

  try {
    // Check for wallet extension
    // In a real implementation, this would use @polkadot/extension-dapp
    // const extensions = await web3Enable('Provider Dashboard')
    // const accounts = await web3Accounts()

    // For now, simulate with test accounts
    await new Promise((resolve) => setTimeout(resolve, 500))

    const testAccounts: Account[] = [
      {
        address: '5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY',
        name: 'Alice',
        type: 'sr25519',
        source: 'local',
      },
      {
        address: '5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty',
        name: 'Bob',
        type: 'sr25519',
        source: 'local',
      },
      {
        address: '5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y',
        name: 'Charlie (New Provider)',
        type: 'sr25519',
        source: 'local',
      },
    ]

    accounts$.next(testAccounts)
    walletStatus$.next('connected')

    // Auto-select first account
    if (testAccounts.length > 0 && !selectedAddress$.getValue()) {
      await selectAccount(testAccounts[0].address)
    }
  } catch (error) {
    walletStatus$.next('error')
    walletError$.next(error instanceof Error ? error.message : 'Wallet connection failed')
    throw error
  }
}

/**
 * Select an account and check its registration status
 */
export async function selectAccount(address: string): Promise<void> {
  const accounts = accounts$.getValue()
  if (!accounts.find((a) => a.address === address)) {
    throw new Error('Account not found')
  }

  selectedAddress$.next(address)

  // Fetch balance and check registration in parallel
  await Promise.all([
    refreshBalance(address),
    checkRegistrationStatus(address),
  ])
}

/**
 * Check if an account is registered as a provider
 */
export async function checkRegistrationStatus(address: string): Promise<boolean> {
  try {
    const isRegistered = await isProviderRegistered(address)

    const current = registrationStatus$.getValue()
    const updated = new Map(current)
    updated.set(address, isRegistered)
    registrationStatus$.next(updated)

    return isRegistered
  } catch (error) {
    console.error('Failed to check registration status:', error)
    return false
  }
}

/**
 * Refresh account balance from chain
 */
export async function refreshBalance(address: string): Promise<void> {
  try {
    const balance = await getAccountBalance(address)
    if (balance) {
      const current = balances$.getValue()
      const updated = new Map(current)
      updated.set(address, balance)
      balances$.next(updated)
    }
  } catch (error) {
    console.error('Failed to fetch balance:', error)
  }
}

/**
 * Disconnect wallet
 */
export function disconnectWallet(): void {
  accounts$.next([])
  selectedAddress$.next(null)
  balances$.next(new Map())
  registrationStatus$.next(new Map())
  walletStatus$.next('disconnected')
  walletError$.next(undefined)
}

/**
 * Mark an account as registered (called after successful registration)
 */
export function markAsRegistered(address: string): void {
  const current = registrationStatus$.getValue()
  const updated = new Map(current)
  updated.set(address, true)
  registrationStatus$.next(updated)
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Get selected address (non-reactive)
 */
export function getSelectedAddress(): string | null {
  return selectedAddress$.getValue()
}

/**
 * Check if wallet is connected (non-reactive)
 */
export function isWalletConnected(): boolean {
  return walletStatus$.getValue() === 'connected'
}

// Export for testing
export const walletActions = {
  connectWallet,
  selectAccount,
  disconnectWallet,
  refreshBalance,
  checkRegistrationStatus,
  markAsRegistered,
  setAccounts: (accounts: Account[]) => accounts$.next(accounts),
  setSelectedAddress: (address: string | null) => selectedAddress$.next(address),
}
