// SPDX-License-Identifier: GPL-3.0-only
//
// Headless PAPI helpers for the Photos scripts — a TypeScript port of the bits
// of `examples/papi/common.js` the deploy + flow scripts need, using the
// workspace's Revive-inclusive descriptors (`@polkadot-api/descriptors`).

import { createClient, type PolkadotClient, type PolkadotSigner } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { getPolkadotSigner } from "polkadot-api/signer";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
  ss58Address,
} from "@polkadot-labs/hdkd-helpers";
import { parachain } from "@polkadot-api/descriptors";
import type { ParachainApi } from "@web3-storage/papi";

const devMiniSecret = entropyToMiniSecret(mnemonicToEntropy(DEV_PHRASE));
const deriveSr25519 = sr25519CreateDerive(devMiniSecret);

export interface Signer {
  signer: PolkadotSigner;
  /** Substrate-default (prefix 42, `5…`) SS58 string — fine for negotiate payloads. */
  address: string;
  publicKey: Uint8Array;
  seed: string;
}

/** Derive a dev signer from a SURI path like `//Alice`. */
export function makeSigner(seed: string): Signer {
  const keyPair = deriveSr25519(seed);
  return {
    signer: getPolkadotSigner(keyPair.publicKey, "Sr25519", keyPair.sign),
    address: ss58Address(keyPair.publicKey),
    publicKey: keyPair.publicKey,
    seed,
  };
}

export interface Connection {
  papi: PolkadotClient;
  api: ParachainApi;
}

export function connect(chainWs: string): Connection {
  const papi = createClient(getWsProvider(chainWs));
  const api = papi.getTypedApi(parachain);
  return { papi, api };
}

/**
 * Storage reads target the best block: `submitTx` returns at in-block
 * inclusion, so a finalized-head read (the default) would miss a just-written
 * value.
 */
export const READ_OPTS = { at: "best" } as const;

export const DEFAULT_TX_TIMEOUT_MS = 180_000;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function formatDispatchError(err: any): string {
  if (!err || typeof err !== "object") return String(err);
  let s = err.type ?? "DispatchError";
  if (err.value?.type) s += `::${err.value.type}`;
  if (err.value?.value?.type) s += `::${err.value.value.type}`;
  return s;
}

/**
 * Sign + submit a tx, resolve at in-block inclusion with the event payload
 * (`{ ok, events, ... }`), reject on dispatch error or timeout. Retries on a
 * stale nonce (lagging client view). Ported from `common.js::submitTx`.
 */
export async function submitTx(
  tx: { signSubmitAndWatch: (signer: PolkadotSigner) => any },
  signer: PolkadotSigner,
  label: string,
  timeoutMs = DEFAULT_TX_TIMEOUT_MS,
): Promise<any> {
  const STALE_RETRIES = 3;
  const STALE_RETRY_DELAY_MS = 6_500;
  for (let attempt = 1; attempt <= STALE_RETRIES; attempt++) {
    try {
      return await watch(tx, signer, label, timeoutMs);
    } catch (err: any) {
      const isStale = /Invalid.*Stale|Stale.*nonce/i.test(err?.message ?? "");
      if (!isStale || attempt === STALE_RETRIES) throw err;
      console.warn(`  ⚠ ${label}: stale nonce (attempt ${attempt}/${STALE_RETRIES}), waiting for next block…`);
      await sleep(STALE_RETRY_DELAY_MS);
    }
  }
}

function watch(
  tx: { signSubmitAndWatch: (signer: PolkadotSigner) => any },
  signer: PolkadotSigner,
  label: string,
  timeoutMs: number,
): Promise<any> {
  return new Promise((resolve, reject) => {
    let done = false;
    let sub: { unsubscribe: () => void } | undefined;
    const cleanup = () => {
      done = true;
      clearTimeout(timer);
      sub?.unsubscribe();
    };
    const timer = setTimeout(() => {
      if (!done) {
        cleanup();
        reject(new Error(`${label}: timed out after ${timeoutMs}ms waiting for in-block`));
      }
    }, timeoutMs);

    sub = tx.signSubmitAndWatch(signer).subscribe({
      next: (ev: any) => {
        if (done) return;
        const failed =
          (ev.type === "txBestBlocksState" && ev.found && ev.ok === false) ||
          (ev.type === "finalized" && ev.ok === false);
        if (failed) {
          cleanup();
          reject(new Error(`${label} dispatch failed: ${formatDispatchError(ev.dispatchError)}`));
          return;
        }
        if (ev.type === "txBestBlocksState" && ev.found) {
          console.log(`📦 ${label} included in block ${ev.block.hash}`);
          cleanup();
          resolve(ev);
        }
      },
      error: (err: any) => {
        if (done) return;
        cleanup();
        reject(new Error(`${label} stream error: ${err}`));
      },
    });
  });
}

/**
 * Assert exactly one event of a kind fired, return its decoded `payload`.
 * (PAPI's typed `event.filter` returns elements shaped `{ payload, ... }`.)
 */
export function requireOneEvent(
  events: any[],
  eventDescriptor: { filter: (events: any[]) => any[] },
  name: string,
): any {
  const matched = eventDescriptor.filter(events);
  if (matched.length !== 1) {
    throw new Error(`Expected exactly 1 ${name} event, got ${matched.length}`);
  }
  return matched[0].payload;
}

/** Verify the runtime is reachable (guards the post-startup metadata window). */
export async function waitForChainReady(api: ParachainApi, { maxRetries = 10, retryDelayMs = 2_000 } = {}): Promise<void> {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      const spec = await api.constants.System.Version();
      console.log(`✅ Chain ready: ${spec.spec_name} v${spec.spec_version}`);
      return;
    } catch (error: any) {
      if (attempt === maxRetries) throw new Error(`Chain not ready after ${maxRetries} attempts: ${error?.message ?? error}`);
      await sleep(retryDelayMs);
    }
  }
}

/** Wait until the chain advances one block (shrinks the dev-key nonce-collision window). */
export function waitForNextBlock(papi: PolkadotClient): Promise<void> {
  return new Promise((resolve) => {
    let initial: number | null = null;
    let sub: { unsubscribe: () => void } | undefined;
    sub = papi.bestBlocks$.subscribe((blocks) => {
      const block = blocks[blocks.length - 1];
      if (initial === null) {
        initial = block.number;
        return;
      }
      if (block.number > initial) {
        sub?.unsubscribe();
        resolve();
      }
    });
  });
}
