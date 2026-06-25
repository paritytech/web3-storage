// SPDX-License-Identifier: GPL-3.0-only

/**
 * Wallet State — network-aware wallet connection.
 *
 * Connection modes:
 * - Dev: well-known dev accounts (Alice, Bob, …) — NO REAL MONEY, local only.
 * - Extension: browser extension (Polkadot.js, Talisman, SubWallet) for testnets.
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
import { getSs58Prefix, isSameAddress, setSs58Prefix, toSs58 } from '@web3-storage/papi'

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

const STORAGE_KEY_MODE = 'photos-wallet-mode'
const STORAGE_KEY_EXTENSION = 'photos-wallet-extension'
const STORAGE_KEY_ACCOUNT = 'photos-wallet-account'

// ─────────────────────────────────────────────────────────────────────────────
// SS58 prefix update (called after the chain connects)
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Update the SS58 prefix from the runtime and re-encode dev accounts so their
 * displayed addresses match the chain's prefix. Called from chain.state after
 * getChainProperties() resolves.
 */
export async function updateSs58Prefix(prefix: number): Promise<void> {
  if (prefix === getSs58Prefix()) return
  setSs58Prefix(prefix)

  if (modeSubject.getValue() === 'dev' && accountsSubject.getValue().length > 0) {
    await connectDevAccounts()
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dev account creation (local development — NO REAL MONEY)
// ─────────────────────────────────────────────────────────────────────────────

function createDevAccountsWithKnownAddresses(): InjectedPolkadotAccount[] {
  try {
    const entropy = mnemonicToEntropy(DEV_PHRASE)
    const miniSecret = entropyToMiniSecret(entropy)
    const derive = sr25519CreateDerive(miniSecret)

    return DEV_ACCOUNT_SEEDS.map(({ name, path }) => {
      const keypair = derive(path)
      const publicKey = keypair.publicKey
      const polkadotSigner = getPolkadotSigner(publicKey, 'Sr25519', (input) => keypair.sign(input))
      return {
        address: toSs58(publicKey),
        name: `${name} (Dev)`,
        polkadotSigner,
      }
    })
  } catch (error) {
    console.error('Failed to create dev accounts:', error)
    return []
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Connection
// ─────────────────────────────────────────────────────────────────────────────

export async function connectDevAccounts(): Promise<void> {
  statusSubject.next('connecting')
  errorSubject.next(undefined)
  modeSubject.next('dev')

  try {
    const devAccounts = createDevAccountsWithKnownAddresses()
    if (devAccounts.length === 0) throw new Error('Failed to create dev accounts')

    accountsSubject.next(devAccounts)
    connectedExtensionSubject.next(undefined)

    // Restore a previously selected account (byte-level match — a persisted
    // address may have been encoded under a different SS58 prefix), else Alice.
    const savedAddress = localStorage.getItem(STORAGE_KEY_ACCOUNT)
    const savedAccount = savedAddress
      ? devAccounts.find((a) => isSameAddress(a.address, savedAddress))
      : undefined
    const accountToSelect = savedAccount ?? devAccounts.find((a) => a.name?.includes('Alice'))
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

export function refreshExtensions(): string[] {
  const extensions = getInjectedExtensions()
  extensionsSubject.next(extensions)
  return extensions
}

export async function connectExtension(extensionName: string): Promise<void> {
  statusSubject.next('connecting')
  errorSubject.next(undefined)
  modeSubject.next('extension')

  try {
    const extension = await connectInjectedExtension(extensionName)
    connectedExtensionSubject.next(extension)

    const accounts = extension.getAccounts()
    accountsSubject.next(accounts)

    const savedAddress = localStorage.getItem(STORAGE_KEY_ACCOUNT)
    const savedAccount = savedAddress
      ? accounts.find((a) => isSameAddress(a.address, savedAddress))
      : undefined

    if (!selectedAccountSubject.getValue()) {
      const accountToSelect = savedAccount ?? accounts[0]
      if (accountToSelect) {
        await selectAccount(accountToSelect.address)
      }
    }

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

export async function selectAccount(address: string): Promise<void> {
  const accounts = accountsSubject.getValue()
  const account = accounts.find((a) => a.address === address)
  if (account) {
    selectedAccountSubject.next(account)
    localStorage.setItem(STORAGE_KEY_ACCOUNT, address)
  }
}

export function disconnectWallet(): void {
  const extension = connectedExtensionSubject.getValue()
  if (extension) extension.disconnect()

  connectedExtensionSubject.next(undefined)
  accountsSubject.next([])
  selectedAccountSubject.next(undefined)
  statusSubject.next('disconnected')
  errorSubject.next(undefined)

  localStorage.removeItem(STORAGE_KEY_MODE)
  localStorage.removeItem(STORAGE_KEY_EXTENSION)
  localStorage.removeItem(STORAGE_KEY_ACCOUNT)
}

/** Auto-reconnect on page load. Defaults to dev mode for local development. */
export async function restoreWalletConnection(): Promise<void> {
  const savedMode = localStorage.getItem(STORAGE_KEY_MODE) as WalletMode | null

  if (!savedMode || savedMode === 'dev') {
    try {
      await connectDevAccounts()
    } catch {
      console.warn('Failed to restore dev accounts')
    }
    return
  }

  if (savedMode === 'extension') {
    const savedExtension = localStorage.getItem(STORAGE_KEY_EXTENSION)
    if (!savedExtension) return

    // Wait briefly for extensions to inject.
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
// Derived state + hooks
// ─────────────────────────────────────────────────────────────────────────────

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
