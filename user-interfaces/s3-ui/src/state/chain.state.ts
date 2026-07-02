// SPDX-License-Identifier: Apache-2.0

/**
 * Chain State - Blockchain connection and block tracking.
 *
 * RxJS BehaviorSubjects + bind() hooks. Network-aware via network.state.ts.
 * Patterned after user-interfaces/provider/src/state/chain.state.ts.
 */

import { BehaviorSubject, map, type Subscription } from "rxjs";
import { bind } from "@react-rxjs/core";
import { createClient, type PolkadotClient, type TypedApi } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws";
import { parachain } from "@polkadot-api/descriptors";
import { setSs58Prefix } from "@web3-storage/sdk";
import { loadSelectedNetwork } from "@web3-storage/network-config";

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "error";

export type ParachainApi = TypedApi<typeof parachain>;

const initialNetwork = loadSelectedNetwork();

const connectionStatus$ = new BehaviorSubject<ConnectionStatus>("disconnected");
const blockNumber$ = new BehaviorSubject<number>(0);
const endpoint$ = new BehaviorSubject<string>(initialNetwork.config.parachainWs);
const connectionError$ = new BehaviorSubject<string | undefined>(undefined);
const client$ = new BehaviorSubject<PolkadotClient | null>(null);
const api$ = new BehaviorSubject<ParachainApi | null>(null);

let blockSubscription: Subscription | null = null;

export const [useConnectionStatus] = bind(connectionStatus$, "disconnected");
export const [useBlockNumber] = bind(blockNumber$, 0);
export const [useEndpoint] = bind(endpoint$, initialNetwork.config.parachainWs);
export const [useConnectionError] = bind(connectionError$, undefined);
export const [useIsConnected] = bind(
  connectionStatus$.pipe(map((s) => s === "connected")),
  false,
);
export const [useIsConnecting] = bind(
  connectionStatus$.pipe(map((s) => s === "connecting")),
  false,
);

export async function connect(wsEndpoint?: string): Promise<void> {
  const ep = wsEndpoint || endpoint$.getValue();

  if (connectionStatus$.getValue() === "connected" && endpoint$.getValue() === ep) {
    return;
  }

  if (client$.getValue()) {
    disconnect();
  }

  endpoint$.next(ep);
  connectionStatus$.next("connecting");
  connectionError$.next(undefined);

  try {
    const provider = getWsProvider(ep);
    const client = createClient(provider);
    const api = client.getTypedApi(parachain);

    blockSubscription = client.finalizedBlock$.subscribe((block) => {
      blockNumber$.next(block.number);
    });

    client$.next(client);
    api$.next(api);

    // Render addresses with the runtime's SS58 prefix (per-network). Dev-account
    // addresses are re-derived lazily on every setSigner(), so they pick this up.
    try {
      setSs58Prefix(await api.constants.System.SS58Prefix());
    } catch {
      /* keep the default prefix */
    }

    connectionStatus$.next("connected");
  } catch (error) {
    connectionStatus$.next("error");
    connectionError$.next(
      error instanceof Error ? error.message : "Connection failed",
    );
    throw error;
  }
}

export function disconnect(): void {
  if (blockSubscription) {
    blockSubscription.unsubscribe();
    blockSubscription = null;
  }

  const client = client$.getValue();
  if (client) {
    try {
      client.destroy();
    } catch {
      /* already destroyed */
    }
  }

  client$.next(null);
  api$.next(null);
  blockNumber$.next(0);
  connectionStatus$.next("disconnected");
}

export async function reconnect(newEndpoint: string): Promise<void> {
  disconnect();
  await connect(newEndpoint);
}

export function getClient(): PolkadotClient | null {
  return client$.getValue();
}

export function getApi(): ParachainApi | null {
  return api$.getValue();
}

export function getEndpoint(): string {
  return endpoint$.getValue();
}

export function isConnected(): boolean {
  return connectionStatus$.getValue() === "connected";
}

export const client$$ = client$.asObservable();
export const api$$ = api$.asObservable();
export const endpointObs$ = endpoint$.asObservable();
