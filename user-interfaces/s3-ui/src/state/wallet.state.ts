// SPDX-License-Identifier: GPL-3.0-only

/**
 * Wallet State - Signer (account) selection.
 *
 * Drive-ui keeps the existing dev-account-by-seed model (Alice/Bob/Charlie/Dave
 * + custom seed input) since browser-extension flows aren't required for the
 * common case. Account NAME is persisted to localStorage so the user comes back
 * to the same identity on reload (the seed is re-derived on hydration for the
 * known dev paths; custom seeds are not auto-restored).
 */

import { BehaviorSubject, combineLatest, map } from "rxjs";
import { bind } from "@react-rxjs/core";
import { getPolkadotSigner } from "polkadot-api/signer";
import { getApi } from "@/state/chain.state";
import { seedToKeypair, toSs58, type Keypair } from "@web3-storage/sdk";

export type Signer = ReturnType<typeof getPolkadotSigner>;

export interface DevAccount {
  name: string;
  seed: string;
  color: string;
}

export const DEV_ACCOUNTS: DevAccount[] = [
  { name: "Alice", seed: "//Alice", color: "bg-indigo-100 text-indigo-700 border-indigo-200" },
  { name: "Bob", seed: "//Bob", color: "bg-emerald-100 text-emerald-700 border-emerald-200" },
  { name: "Charlie", seed: "//Charlie", color: "bg-amber-100 text-amber-700 border-amber-200" },
  { name: "Dave", seed: "//Dave", color: "bg-rose-100 text-rose-700 border-rose-200" },
];

export interface AccountBalance {
  free: bigint;
  reserved: bigint;
}

const STORAGE_KEY_ACCOUNT_NAME = "drive-ui-account-name";

const signer$ = new BehaviorSubject<Signer | null>(null);
const keypair$ = new BehaviorSubject<Keypair | null>(null);
const signerAddress$ = new BehaviorSubject<string | null>(null);
const signerName$ = new BehaviorSubject<string | null>(null);
const balance$ = new BehaviorSubject<AccountBalance | null>(null);
const settingSigner$ = new BehaviorSubject<boolean>(false);

export const [useSignerAddress] = bind(signerAddress$, null);
export const [useSignerName] = bind(signerName$, null);
export const [useBalance] = bind(balance$, null);
export const [useIsSettingSigner] = bind(settingSigner$, false);

export const [useHasSigner] = bind(
  signerAddress$.pipe(map((a) => a !== null)),
  false,
);

export async function setSigner(seed: string, name?: string): Promise<string> {
  settingSigner$.next(true);
  try {
    const keypair = seedToKeypair(seed);
    const address = toSs58(keypair.publicKey);

    const newSigner = getPolkadotSigner(keypair.publicKey, "Sr25519", (input) =>
      keypair.sign(input),
    );

    signer$.next(newSigner);
    keypair$.next(keypair);
    signerAddress$.next(address);
    signerName$.next(name ?? null);

    if (name) {
      localStorage.setItem(STORAGE_KEY_ACCOUNT_NAME, name);
    } else {
      localStorage.removeItem(STORAGE_KEY_ACCOUNT_NAME);
    }

    await refreshBalance();
    return address;
  } finally {
    settingSigner$.next(false);
  }
}

export async function selectDevAccount(name: string): Promise<void> {
  const dev = DEV_ACCOUNTS.find((a) => a.name === name);
  if (!dev) throw new Error(`Unknown dev account: ${name}`);
  await setSigner(dev.seed, dev.name);
}

export function clearSigner(): void {
  signer$.next(null);
  keypair$.next(null);
  signerAddress$.next(null);
  signerName$.next(null);
  balance$.next(null);
  localStorage.removeItem(STORAGE_KEY_ACCOUNT_NAME);
}

export async function refreshBalance(): Promise<void> {
  const api = getApi();
  const address = signerAddress$.getValue();
  if (!api || !address) return;

  try {
    const account = await api.query.System.Account.getValue(address);
    balance$.next({ free: account.data.free, reserved: account.data.reserved });
  } catch {
    /* leave previous balance in place on transient failure */
  }
}

/**
 * Restore signer from persisted account name. Only restores known dev accounts;
 * custom seeds are intentionally not persisted.
 */
export async function restoreSigner(): Promise<void> {
  const savedName = localStorage.getItem(STORAGE_KEY_ACCOUNT_NAME);
  if (!savedName) return;
  const dev = DEV_ACCOUNTS.find((a) => a.name === savedName);
  if (!dev) return;
  try {
    await setSigner(dev.seed, dev.name);
  } catch {
    localStorage.removeItem(STORAGE_KEY_ACCOUNT_NAME);
  }
}

export function getSigner(): Signer | null {
  return signer$.getValue();
}

export function getKeypair(): Keypair | null {
  return keypair$.getValue();
}

export function getSignerAddress(): string | null {
  return signerAddress$.getValue();
}

export const signer$$ = signer$.asObservable();
export const keypair$$ = keypair$.asObservable();
export const signerAddress$$ = signerAddress$.asObservable();

export const signerInfo$ = combineLatest([signerAddress$, signerName$]).pipe(
  map(([address, name]) => ({ address, name })),
);
