// SPDX-License-Identifier: GPL-3.0-only

/**
 * Wallet State — the product account of the TrUAPI host photos runs in. The
 * host owns the account and signs for it, so photos only reads it.
 */

import { BehaviorSubject, combineLatest, of, switchMap, map, catchError, type Observable } from 'rxjs'
import { bind } from '@react-rxjs/core'
import type { InjectedPolkadotAccount } from 'polkadot-api/pjs-signer'
import { fromHex } from 'polkadot-api/utils'
import { getSs58Prefix, setSs58Prefix, toSs58 } from '@web3-storage/papi'
import { clientReady$, getClient, requireApi } from '@/lib/chain-client'
import { getHostAccount, isInHost } from '@/lib/host-account'

export type WalletStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

const statusSubject = new BehaviorSubject<WalletStatus>('disconnected')
const errorSubject = new BehaviorSubject<string | undefined>(undefined)
const accountSubject = new BehaviorSubject<InjectedPolkadotAccount | undefined>(undefined)

export const [useWalletStatus] = bind(statusSubject, 'disconnected')
export const [useWalletError] = bind(errorSubject, undefined)
export const [useSelectedAccount] = bind(accountSubject, undefined)

/**
 * The free balance of the host account at the best block, or `undefined`
 * while the chain or the account is not ready or the read fails.
 */
const freeBalance$ = combineLatest([accountSubject, clientReady$]).pipe(
  switchMap(([account, ready]) => {
    if (!account || !ready || !getClient()) return of(undefined)
    return watchFreeBalance(account.address)
  }),
)

export const [useFreeBalance] = bind(freeBalance$, undefined)

/**
 * The public key of the account that owns photos' bucket.
 *
 * TODO: the host derives this account and does not share it with products.
 * Remove this constant once the host reports the bucket balance itself.
 */
const BUCKET_ACCOUNT = '0xf60b308020337868c228312414208e5cede457b6e7aa3f93a1fe0715291f646f'

/**
 * The free balance of the bucket account at the best block, or `undefined`
 * while the chain is not ready or the read fails.
 */
const bucketFreeBalance$ = clientReady$.pipe(
  switchMap((ready) => {
    if (!ready || !getClient()) return of(undefined)
    return watchFreeBalance(toSs58(fromHex(BUCKET_ACCOUNT)))
  }),
)

export const [useBucketFreeBalance] = bind(bucketFreeBalance$, undefined)

/** The free balance of `address` at the best block, or `undefined` if the read fails. */
function watchFreeBalance(address: string): Observable<bigint | undefined> {
  return requireApi()
    .query.System.Account.watchValue(address, { at: 'best' })
    .pipe(
      map(({ value }) => value.data.free),
      catchError(() => of(undefined)),
    )
}

/**
 * Set the SS58 prefix from the runtime and re-encode the account's address
 * with it. Called after the chain connects.
 */
export function updateSs58Prefix(prefix: number): void {
  if (prefix === getSs58Prefix()) return
  setSs58Prefix(prefix)
  const account = accountSubject.getValue()
  if (account) accountSubject.next({ ...account, address: toSs58(account.txCreator.publicKey) })
}

/** Read the host's product account. Does nothing outside a host. */
export async function connectHostAccount(): Promise<void> {
  if (!isInHost()) return
  statusSubject.next('connecting')
  errorSubject.next(undefined)
  try {
    const { publicKey, txCreator } = await getHostAccount()
    accountSubject.next({ address: toSs58(publicKey), name: 'Host account', txCreator })
    statusSubject.next('connected')
  } catch (err) {
    errorSubject.next(err instanceof Error ? err.message : 'Could not read the host account')
    statusSubject.next('error')
    throw err
  }
}
