import { createClient, type PolkadotClient, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { parachain } from "@polkadot-api/descriptors";
import type { PolkadotSigner } from "polkadot-api/signer";

export type ParachainApi = TypedApi<typeof parachain>;

const DEFAULT_WS = process.env.CHAIN_WS ?? "ws://127.0.0.1:2222";

let cachedClient: PolkadotClient | null = null;
let cachedApi: ParachainApi | null = null;
let cachedWs: string | null = null;

export function getClient(wsUrl: string = DEFAULT_WS): PolkadotClient {
  if (cachedClient && cachedWs === wsUrl) return cachedClient;
  if (cachedClient) cachedClient.destroy();
  cachedClient = createClient(getWsProvider(wsUrl));
  cachedApi = cachedClient.getTypedApi(parachain);
  cachedWs = wsUrl;
  return cachedClient;
}

export function getApi(wsUrl: string = DEFAULT_WS): ParachainApi {
  getClient(wsUrl);
  return cachedApi!;
}

export function disconnect(): void {
  if (cachedClient) {
    try {
      cachedClient.destroy();
    } catch {
      // already destroyed
    }
  }
  cachedClient = null;
  cachedApi = null;
  cachedWs = null;
}

export interface SubmitResult<E = unknown> {
  events: E[];
  blockHash: string;
  txHash: string;
}

/**
 * Sign + submit an extrinsic and wait for inclusion. Throws if any
 * `System.ExtrinsicFailed` event is in the result.
 *
 * `tx` is whatever `api.tx.Pallet.method({...})` returns.
 */
export async function submitExtrinsic<T extends { signAndSubmit: (s: PolkadotSigner) => Promise<unknown> }>(
  tx: T,
  signer: PolkadotSigner,
): Promise<SubmitResult> {
  const result = (await tx.signAndSubmit(signer)) as SubmitResult;
  const api = getApi();
  const failed = api.event.System.ExtrinsicFailed.filter(result.events as never);
  if (failed.length > 0) {
    const err = JSON.stringify(failed[0], (_, v) =>
      typeof v === "bigint" ? v.toString() : v,
    );
    throw new Error(`Extrinsic failed: ${err}`);
  }
  return result;
}

/**
 * Wait until the chain has produced a block at or after `target`.
 * Useful when a test needs to wait for a checkpoint window etc.
 */
export async function waitForBlock(target: number, timeoutMs = 120_000): Promise<void> {
  const client = getClient();
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => {
      sub.unsubscribe();
      reject(new Error(`Timed out waiting for block ${target} after ${timeoutMs}ms`));
    }, timeoutMs);
    const sub = client.finalizedBlock$.subscribe((b) => {
      if (b.number >= target) {
        clearTimeout(t);
        sub.unsubscribe();
        resolve();
      }
    });
  });
}

export async function getBlockNumber(): Promise<number> {
  const block = await getClient().getFinalizedBlock();
  return block.number;
}
