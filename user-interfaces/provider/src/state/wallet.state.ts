/**
 * Wallet State - Network-aware wallet connection
 *
 * Connection modes:
 * - Local/Dev: Uses well-known dev accounts (Alice, Bob, etc.) - NO REAL MONEY
 * - Testnet: Uses browser extensions with faucet-funded accounts
 * - Mainnet: Uses browser extensions with real accounts
 *
 * The mode is determined by the connected chain endpoint.
 */

import { BehaviorSubject, combineLatest, map, shareReplay } from 'rxjs'
import { bind } from '@react-rxjs/core'
import {
  connectInjectedExtension,
  getInjectedExtensions,
  InjectedExtension,
  InjectedPolkadotAccount,
} from 'polkadot-api/pjs-signer'
import { sr25519CreateDerive } from '@polkadot-labs/hdkd'
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
} from '@polkadot-labs/hdkd-helpers'
import { getPolkadotSigner } from 'polkadot-api/signer'
import { getAccountBalance, isProviderRegistered, DEV_ACCOUNTS as DEV_ACCOUNT_ADDRESSES } from '@/lib/chain-client'

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

export type WalletMode = 'dev' | 'extension'

export interface WalletState {
  mode: WalletMode
  status: 'disconnected' | 'connecting' | 'connected' | 'error'
  error?: string
  extensions: string[]
  connectedExtension?: InjectedExtension
  accounts: InjectedPolkadotAccount[]
  selectedAccount?: InjectedPolkadotAccount
}

export interface AccountBalance {
  free: bigint
  reserved: bigint
  frozen: bigint
}

// Well-known development accounts (from Substrate)
const DEV_ACCOUNT_SEEDS = [
  { name: 'Alice', path: '//Alice' },
  { name: 'Bob', path: '//Bob' },
  { name: 'Charlie', path: '//Charlie' },
  { name: 'Dave', path: '//Dave' },
  { name: 'Eve', path: '//Eve' },
  { name: 'Ferdie', path: '//Ferdie' },
]

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

const modeSubject = new BehaviorSubject<WalletMode>('dev')
const statusSubject = new BehaviorSubject<WalletState['status']>('disconnected')
const errorSubject = new BehaviorSubject<string | undefined>(undefined)
const extensionsSubject = new BehaviorSubject<string[]>([])
const connectedExtensionSubject = new BehaviorSubject<InjectedExtension | undefined>(undefined)
const accountsSubject = new BehaviorSubject<InjectedPolkadotAccount[]>([])
const selectedAccountSubject = new BehaviorSubject<InjectedPolkadotAccount | undefined>(undefined)

// Balance and registration status
const balancesSubject = new BehaviorSubject<Map<string, AccountBalance>>(new Map())
const registrationStatusSubject = new BehaviorSubject<Map<string, boolean>>(new Map())

// LocalStorage keys
const STORAGE_KEY_MODE = 'provider-dashboard-wallet-mode'
const STORAGE_KEY_EXTENSION = 'provider-dashboard-wallet-extension'
const STORAGE_KEY_ACCOUNT = 'provider-dashboard-wallet-account'

// ─────────────────────────────────────────────────────────────────────────────
// Dev Account Creation (for local development - NO REAL MONEY)
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Create dev accounts from the well-known development seed phrase.
 * Uses known SS58 addresses for standard dev accounts (Alice, Bob, etc.)
 * These accounts are ONLY for local development and have no real value.
 */
function createDevAccountsWithKnownAddresses(): InjectedPolkadotAccount[] {
  try {
    const entropy = mnemonicToEntropy(DEV_PHRASE)
    const miniSecret = entropyToMiniSecret(entropy)
    const derive = sr25519CreateDerive(miniSecret)

    return DEV_ACCOUNT_SEEDS.map(({ name, path }) => {
      const keypair = derive(path)
      const publicKey = keypair.publicKey

      const polkadotSigner = getPolkadotSigner(publicKey, 'Sr25519', (input) =>
        keypair.sign(input)
      )

      return {
        address: DEV_ACCOUNT_ADDRESSES[name] || '',
        name: `${name} (Dev)`,
        polkadotSigner,
      }
    }).filter((account) => account.address !== '')
  } catch (error) {
    console.error('Failed to create dev accounts:', error)
    return []
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Connection Functions
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Connect using dev accounts (for local development)
 * These accounts have NO REAL VALUE - only for testing
 */
export async function connectDevAccounts(): Promise<void> {
  statusSubject.next('connecting')
  errorSubject.next(undefined)
  modeSubject.next('dev')

  try {
    const devAccounts = createDevAccountsWithKnownAddresses()

    if (devAccounts.length === 0) {
      throw new Error('Failed to create dev accounts')
    }

    accountsSubject.next(devAccounts)
    connectedExtensionSubject.next(undefined)

    // Restore previously selected account if it's a known dev account, else
    // default to Alice. Mirrors the extension-mode hydration so a persisted
    // choice survives reload.
    const savedAddress = localStorage.getItem(STORAGE_KEY_ACCOUNT)
    const savedAccount = savedAddress
      ? devAccounts.find((a) => a.address === savedAddress)
      : undefined
    const accountToSelect =
      savedAccount ?? devAccounts.find((a) => a.name?.includes('Alice'))
    if (accountToSelect) {
      await selectAccount(accountToSelect.address)
    }

    localStorage.setItem(STORAGE_KEY_MODE, 'dev')
    statusSubject.next('connected')
  } catch (err) {
    const message = err instanceof Error ? err.message : 'Failed to connect dev accounts'
    errorSubject.next(message)
    statusSubject.next('error')
    throw err
  }
}

/**
 * Refresh the list of available wallet extensions
 */
export function refreshExtensions(): string[] {
  const extensions = getInjectedExtensions()
  extensionsSubject.next(extensions)
  return extensions
}

/**
 * Connect to a browser wallet extension (for testnet/mainnet)
 */
export async function connectExtension(extensionName: string): Promise<void> {
  statusSubject.next('connecting')
  errorSubject.next(undefined)
  modeSubject.next('extension')

  try {
    const extension = await connectInjectedExtension(extensionName)
    connectedExtensionSubject.next(extension)

    const accounts = extension.getAccounts()
    accountsSubject.next(accounts)

    // Restore previously selected account, or auto-select first
    const savedAddress = localStorage.getItem(STORAGE_KEY_ACCOUNT)
    const savedAccount = savedAddress
      ? accounts.find((a) => a.address === savedAddress)
      : undefined

    if (!selectedAccountSubject.getValue()) {
      const accountToSelect = savedAccount ?? accounts[0]
      if (accountToSelect) {
        await selectAccount(accountToSelect.address)
      }
    }

    // Subscribe to account changes
    extension.subscribe((newAccounts) => {
      accountsSubject.next(newAccounts)
      const selected = selectedAccountSubject.getValue()
      if (selected && !newAccounts.find((a) => a.address === selected.address)) {
        if (newAccounts[0]) {
          selectAccount(newAccounts[0].address)
        } else {
          selectedAccountSubject.next(undefined)
        }
      }
    })

    localStorage.setItem(STORAGE_KEY_MODE, 'extension')
    localStorage.setItem(STORAGE_KEY_EXTENSION, extensionName)
    statusSubject.next('connected')
  } catch (err) {
    const message = err instanceof Error ? err.message : 'Failed to connect wallet'
    errorSubject.next(message)
    statusSubject.next('error')
    throw err
  }
}

/**
 * Select an account and fetch its balance/registration status
 */
export async function selectAccount(address: string): Promise<void> {
  const accounts = accountsSubject.getValue()
  const account = accounts.find((a) => a.address === address)

  if (account) {
    selectedAccountSubject.next(account)
    localStorage.setItem(STORAGE_KEY_ACCOUNT, address)

    // Fetch balance and registration status in parallel
    await Promise.all([refreshBalance(address), checkRegistrationStatus(address)])
  }
}

/**
 * Disconnect wallet
 */
export function disconnectWallet(): void {
  const extension = connectedExtensionSubject.getValue()
  if (extension) {
    extension.disconnect()
  }

  connectedExtensionSubject.next(undefined)
  accountsSubject.next([])
  selectedAccountSubject.next(undefined)
  balancesSubject.next(new Map())
  registrationStatusSubject.next(new Map())
  statusSubject.next('disconnected')
  errorSubject.next(undefined)

  localStorage.removeItem(STORAGE_KEY_MODE)
  localStorage.removeItem(STORAGE_KEY_EXTENSION)
  localStorage.removeItem(STORAGE_KEY_ACCOUNT)
}

/**
 * Auto-reconnect on page load
 * Defaults to dev mode for local development
 */
export async function restoreWalletConnection(): Promise<void> {
  const savedMode = localStorage.getItem(STORAGE_KEY_MODE) as WalletMode | null

  // Default to dev mode if no saved mode (safest for development)
  if (!savedMode || savedMode === 'dev') {
    try {
      await connectDevAccounts()
    } catch {
      console.warn('Failed to restore dev accounts')
    }
    return
  }

  // Extension mode
  if (savedMode === 'extension') {
    const savedExtension = localStorage.getItem(STORAGE_KEY_EXTENSION)
    if (!savedExtension) return

    // Wait briefly for extensions to inject
    await new Promise((resolve) => setTimeout(resolve, 200))

    const available = getInjectedExtensions()
    extensionsSubject.next(available)

    if (available.includes(savedExtension)) {
      try {
        await connectExtension(savedExtension)
      } catch {
        localStorage.removeItem(STORAGE_KEY_MODE)
        localStorage.removeItem(STORAGE_KEY_EXTENSION)
        localStorage.removeItem(STORAGE_KEY_ACCOUNT)
      }
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Balance & Registration
// ─────────────────────────────────────────────────────────────────────────────

export async function refreshBalance(address: string): Promise<void> {
  try {
    const balance = await getAccountBalance(address)
    if (balance) {
      const current = balancesSubject.getValue()
      const updated = new Map(current)
      updated.set(address, balance)
      balancesSubject.next(updated)
    }
  } catch (error) {
    console.error('Failed to fetch balance:', error)
  }
}

export async function checkRegistrationStatus(address: string): Promise<boolean> {
  try {
    const isRegistered = await isProviderRegistered(address)

    const current = registrationStatusSubject.getValue()
    const updated = new Map(current)
    updated.set(address, isRegistered)
    registrationStatusSubject.next(updated)

    return isRegistered
  } catch (error) {
    console.error('Failed to check registration status:', error)
    return false
  }
}

export function markAsRegistered(address: string): void {
  const current = registrationStatusSubject.getValue()
  const updated = new Map(current)
  updated.set(address, true)
  registrationStatusSubject.next(updated)
}

// ─────────────────────────────────────────────────────────────────────────────
// Derived State
// ─────────────────────────────────────────────────────────────────────────────

const selectedBalance$ = combineLatest([balancesSubject, selectedAccountSubject]).pipe(
  map(([balances, account]) => (account ? balances.get(account.address) : null) || null)
)

const isSelectedRegistered$ = combineLatest([
  registrationStatusSubject,
  selectedAccountSubject,
]).pipe(map(([status, account]) => (account ? status.get(account.address) ?? null : null)))

const walletState$ = combineLatest([
  modeSubject,
  statusSubject,
  errorSubject,
  extensionsSubject,
  connectedExtensionSubject,
  accountsSubject,
  selectedAccountSubject,
]).pipe(
  map(([mode, status, error, extensions, connectedExtension, accounts, selectedAccount]) => ({
    mode,
    status,
    error,
    extensions,
    connectedExtension,
    accounts,
    selectedAccount,
  })),
  shareReplay(1)
)

// ─────────────────────────────────────────────────────────────────────────────
// React Hooks
// ─────────────────────────────────────────────────────────────────────────────

export const [useWalletState] = bind(walletState$, {
  mode: 'dev' as const,
  status: 'disconnected' as const,
  error: undefined,
  extensions: [],
  connectedExtension: undefined,
  accounts: [],
  selectedAccount: undefined,
})

export const [useWalletMode] = bind(modeSubject, 'dev')
export const [useWalletStatus] = bind(statusSubject, 'disconnected')
export const [useWalletError] = bind(errorSubject, undefined)
export const [useAvailableExtensions] = bind(extensionsSubject, [])
export const [useAccounts] = bind(accountsSubject, [])
export const [useSelectedAccount] = bind(selectedAccountSubject, undefined)
export const [useSelectedBalance] = bind(selectedBalance$, null)
export const [useIsSelectedRegistered] = bind(isSelectedRegistered$, null)

export const [useAccountBalance] = bind(
  (address: string) => balancesSubject.pipe(map((balances) => balances.get(address) || null)),
  null
)

export const [useIsRegistered] = bind(
  (address: string) =>
    registrationStatusSubject.pipe(map((status) => status.get(address) ?? null)),
  null
)

// ─────────────────────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────────────────────

export function getSelectedAddress(): string | null {
  return selectedAccountSubject.getValue()?.address ?? null
}

export function getSelectedAccount(): InjectedPolkadotAccount | undefined {
  return selectedAccountSubject.getValue()
}

export function isWalletConnected(): boolean {
  return statusSubject.getValue() === 'connected'
}

export function isDevMode(): boolean {
  return modeSubject.getValue() === 'dev'
}

export const selectedAccount$ = selectedAccountSubject.asObservable()
export const accounts$ = accountsSubject.asObservable()

export const walletActions = {
  connectDevAccounts,
  refreshExtensions,
  connectExtension,
  selectAccount,
  disconnectWallet,
  restoreWalletConnection,
  refreshBalance,
  checkRegistrationStatus,
  markAsRegistered,
}
