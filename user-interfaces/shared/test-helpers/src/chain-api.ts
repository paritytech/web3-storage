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
 *
 * Computes the next nonce from the legacy `system_accountNextIndex` JSON-RPC
 * method, bypassing PAPI's chainHead-cached "latest finalized" view. PAPI's
 * default reads nonce from a block on the local chainHead, which can lag
 * behind the chain's actual state when same-signer txs were submitted
 * recently — leading to `Invalid::Stale` rejections. The RPC queries the
 * node directly and accounts for pool state.
 */
export async function submitExtrinsic(
  tx: Transaction,
  signer: PolkadotSigner,
  signerAddress: string,
): Promise<TxFinalizedPayload> {
  const client = getClient();
  const nonce = await client._request<number>("system_accountNextIndex", [signerAddress]);
  const result = await tx.signAndSubmit(signer, { nonce });
  if (!result.ok) {
    const err = JSON.stringify(result.dispatchError, (_, v) =>
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
