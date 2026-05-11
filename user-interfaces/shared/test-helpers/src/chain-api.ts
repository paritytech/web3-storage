import { createClient, type PolkadotClient, type Transaction, type TxFinalizedPayload, type TypedApi } from "polkadot-api";
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

/**
 * Sign + submit an extrinsic, wait for finalization, throw if it failed.
 */
export async function submitExtrinsic(
  tx: Transaction,
  signer: PolkadotSigner,
): Promise<TxFinalizedPayload> {
  const result = await tx.signAndSubmit(signer);
  if (!result.ok) {
    const err = JSON.stringify(result.dispatchError, (_, v) =>
      typeof v === "bigint" ? v.toString() : v,
    );
    throw new Error(`Extrinsic failed: ${err}`);
  }
  return result;
}

/**
 * Sign + submit an extrinsic, resolve as soon as it's included in a best
 * block (~6s) instead of finalized (~12-24s). Use for fire-and-forget
 * cleanup paths where the caller doesn't need finality guarantees and
 * just wants the chain to acknowledge the tx.
 */
export async function submitExtrinsicBestBlock(
  tx: Transaction,
  signer: PolkadotSigner,
): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const sub = tx.signSubmitAndWatch(signer).subscribe({
      next: (ev: any) => {
        if (ev.type === "txBestBlocksState" && ev.found && !settled) {
          settled = true;
          sub.unsubscribe();
          if (ev.ok === false) {
            const err = JSON.stringify(ev.dispatchError, (_, v) =>
              typeof v === "bigint" ? v.toString() : v,
            );
            reject(new Error(`Extrinsic failed at best block: ${err}`));
          } else {
            resolve();
          }
        }
      },
      error: (err) => {
        if (!settled) {
          settled = true;
          reject(err);
        }
      },
    });
  });
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
