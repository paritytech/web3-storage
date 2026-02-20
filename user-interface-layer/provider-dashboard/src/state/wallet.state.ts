import { BehaviorSubject, combineLatest, map } from 'rxjs'
import { bind } from '@react-rxjs/core'

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

// State subjects
const accounts$ = new BehaviorSubject<Account[]>([])
const selectedAddress$ = new BehaviorSubject<string | null>(null)
const balances$ = new BehaviorSubject<Map<string, AccountBalance>>(new Map())
const isConnecting$ = new BehaviorSubject<boolean>(false)

// Derived state
const selectedAccount$ = combineLatest([accounts$, selectedAddress$]).pipe(
  map(([accounts, address]) => accounts.find((a) => a.address === address) || null)
)

const selectedBalance$ = combineLatest([balances$, selectedAddress$]).pipe(
  map(([balances, address]) => (address ? balances.get(address) : null) || null)
)

// React hooks
export const [useAccounts] = bind(accounts$, [])
export const [useSelectedAddress] = bind(selectedAddress$, null)
export const [useSelectedAccount] = bind(selectedAccount$, null)
export const [useSelectedBalance] = bind(selectedBalance$, null)
export const [useIsWalletConnecting] = bind(isConnecting$, false)

export const [useAccountBalance] = bind(
  (address: string) => balances$.pipe(map((balances) => balances.get(address) || null)),
  null
)

// Actions
export async function connectWallet(): Promise<void> {
  isConnecting$.next(true)

  try {
    // TODO: Implement actual wallet connection with polkadot extension
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
    ]

    accounts$.next(testAccounts)

    // Auto-select first account
    if (testAccounts.length > 0 && !selectedAddress$.getValue()) {
      selectedAddress$.next(testAccounts[0].address)
    }

    // Set mock balances
    const mockBalances = new Map<string, AccountBalance>()
    for (const account of testAccounts) {
      mockBalances.set(account.address, {
        free: 10_000_000_000_000_000n, // 10,000 tokens
        reserved: 0n,
        frozen: 0n,
      })
    }
    balances$.next(mockBalances)
  } finally {
    isConnecting$.next(false)
  }
}

export function selectAccount(address: string): void {
  const accounts = accounts$.getValue()
  if (accounts.find((a) => a.address === address)) {
    selectedAddress$.next(address)
  }
}

export function disconnectWallet(): void {
  accounts$.next([])
  selectedAddress$.next(null)
  balances$.next(new Map())
}

export async function refreshBalance(address: string): Promise<void> {
  // TODO: Query chain for actual balance
  // For now, just return existing balance
}

// Export actions for testing
export const walletActions = {
  connectWallet,
  selectAccount,
  disconnectWallet,
  refreshBalance,
  setAccounts: (accounts: Account[]) => accounts$.next(accounts),
  setSelectedAddress: (address: string | null) => selectedAddress$.next(address),
}
