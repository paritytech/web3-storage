// SPDX-License-Identifier: GPL-3.0-only

/**
 * Wallet State — "highlight mine" only.
 *
 * The explorer is read-only: it never signs or submits anything. A wallet is
 * connected purely to know which addresses are the user's, so their rows can
 * be highlighted. Only account *addresses* are kept — no signers.
 *
 * Modes:
 * - dev: the well-known dev accounts (Alice…Ferdie) — useful on local chains
 * - extension: all accounts from a browser wallet extension
 */

import { BehaviorSubject, map } from 'rxjs'
import { bind } from '@react-rxjs/core'
import { connectInjectedExtension, getInjectedExtensions } from 'polkadot-api/pjs-signer'
import { getSs58Prefix, isSameAddress, makeSigner, setSs58Prefix } from '@web3-storage/sdk'

export type WalletMode = 'none' | 'dev' | 'extension'

const DEV_ACCOUNT_PATHS = ['//Alice', '//Bob', '//Charlie', '//Dave', '//Eve', '//Ferdie']

const STORAGE_KEY_MODE = 'explorer-wallet-mode'
const STORAGE_KEY_EXTENSION = 'explorer-wallet-extension'

const mode$ = new BehaviorSubject<WalletMode>('none')
const myAddresses$ = new BehaviorSubject<string[]>([])
const extensions$ = new BehaviorSubject<string[]>([])
const walletError$ = new BehaviorSubject<string | undefined>(undefined)
let extensionDisconnect: (() => void) | null = null

export const [useWalletMode] = bind(mode$, 'none')
export const [useMyAddresses] = bind(myAddresses$, [])
export const [useAvailableExtensions] = bind(extensions$, [])
export const [useWalletError] = bind(walletError$, undefined)

/**
 * Matcher hook for row highlighting. Byte-level comparison because chain
 * queries and extensions may encode the same key under different SS58
 * prefixes. Results are memoized per address — `isSameAddress` base58-decodes
 * both operands, which adds up over thousands of rows re-rendered on every
 * anchor tick. The cache lives inside the matcher closure, so it resets
 * whenever the highlighted set changes.
 */
export const [useIsMine] = bind(
  myAddresses$.pipe(
    map((mine) => {
      const cache = new Map<string, boolean>()
      return (address: string) => {
        let hit = cache.get(address)
        if (hit === undefined) {
          hit = mine.some((m) => isSameAddress(m, address))
          cache.set(address, hit)
        }
        return hit
      }
    })
  ),
  () => false
)

/**
 * Update the SS58 prefix from the runtime and re-derive dev addresses so they
 * compare and display under the chain's encoding. Called from chain.state
 * after the chain connects.
 */
export async function updateSs58Prefix(prefix: number): Promise<void> {
  if (prefix === getSs58Prefix()) return
  setSs58Prefix(prefix)
  if (mode$.getValue() === 'dev') {
    myAddresses$.next(deriveDevAddresses())
  }
}

// The signers makeSigner creates are discarded — only addresses are kept.
function deriveDevAddresses(): string[] {
  return DEV_ACCOUNT_PATHS.map((path) => makeSigner(path).address)
}

// Tear down any live extension connection so its account subscription can't
// keep firing and overwrite the highlighted set after a mode switch.
function teardownExtension(): void {
  if (extensionDisconnect) {
    extensionDisconnect()
    extensionDisconnect = null
  }
}

/** Highlight the well-known dev accounts (local development). */
export function connectDevAccounts(): void {
  teardownExtension()
  walletError$.next(undefined)
  try {
    myAddresses$.next(deriveDevAddresses())
    mode$.next('dev')
    localStorage.setItem(STORAGE_KEY_MODE, 'dev')
  } catch (err) {
    walletError$.next(err instanceof Error ? err.message : 'Failed to derive dev accounts')
  }
}

/** Refresh the list of available wallet extensions. */
export function refreshExtensions(): string[] {
  const extensions = getInjectedExtensions()
  extensions$.next(extensions)
  return extensions
}

/** Highlight all accounts exposed by a browser wallet extension. */
export async function connectExtension(extensionName: string): Promise<void> {
  teardownExtension()
  walletError$.next(undefined)
  try {
    const extension = await connectInjectedExtension(extensionName)
    myAddresses$.next(extension.getAccounts().map((a) => a.address))
    const stopSubscription = extension.subscribe((accounts) => {
      myAddresses$.next(accounts.map((a) => a.address))
    })
    extensionDisconnect = () => {
      if (typeof stopSubscription === 'function') stopSubscription()
      extension.disconnect()
    }
    mode$.next('extension')
    localStorage.setItem(STORAGE_KEY_MODE, 'extension')
    localStorage.setItem(STORAGE_KEY_EXTENSION, extensionName)
  } catch (err) {
    walletError$.next(err instanceof Error ? err.message : 'Failed to connect wallet')
    throw err
  }
}

/** Stop highlighting. */
export function disconnectWallet(): void {
  teardownExtension()
  mode$.next('none')
  myAddresses$.next([])
  walletError$.next(undefined)
  localStorage.removeItem(STORAGE_KEY_MODE)
  localStorage.removeItem(STORAGE_KEY_EXTENSION)
}

/** Restore a persisted highlight choice on page load; default is off. */
export async function restoreWalletConnection(): Promise<void> {
  const savedMode = localStorage.getItem(STORAGE_KEY_MODE) as WalletMode | null

  if (savedMode === 'dev') {
    connectDevAccounts()
    return
  }

  if (savedMode === 'extension') {
    const savedExtension = localStorage.getItem(STORAGE_KEY_EXTENSION)
    if (!savedExtension) return

    // Wait briefly for extensions to inject
    await new Promise((resolve) => setTimeout(resolve, 200))

    if (getInjectedExtensions().includes(savedExtension)) {
      try {
        await connectExtension(savedExtension)
      } catch {
        localStorage.removeItem(STORAGE_KEY_MODE)
        localStorage.removeItem(STORAGE_KEY_EXTENSION)
      }
    }
  }
}
