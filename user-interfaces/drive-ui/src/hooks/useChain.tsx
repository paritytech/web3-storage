import {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  type ReactNode,
} from "react";
import { createClient, type PolkadotClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider/web";
import { BehaviorSubject } from "rxjs";

interface ChainState {
  client: PolkadotClient | null;
  connected: boolean;
  connecting: boolean;
  error: string | null;
  chainEndpoint: string;
  blockNumber: number;
  connect: (chainWs: string) => Promise<void>;
  disconnect: () => void;
}

const defaultState: ChainState = {
  client: null,
  connected: false,
  connecting: false,
  error: null,
  chainEndpoint: "ws://127.0.0.1:2222",
  blockNumber: 0,
  connect: async () => {},
  disconnect: () => {},
};

const ChainContext = createContext<ChainState>(defaultState);

export const blockNumber$ = new BehaviorSubject<number>(0);

export function ChainProvider({ children }: { children: ReactNode }) {
  const [client, setClient] = useState<PolkadotClient | null>(null);
  const [connected, setConnected] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [chainEndpoint, setChainEndpoint] = useState("ws://127.0.0.1:2222");
  const [blockNumber, setBlockNumber] = useState(0);

  const connect = useCallback(
    async (chainWs: string) => {
      if (connecting || connected) return;

      setConnecting(true);
      setError(null);
      setChainEndpoint(chainWs);

      try {
        const provider = getWsProvider(chainWs);
        const newClient = createClient(provider);

        const unsub = newClient.finalizedBlock$.subscribe((block) => {
          setBlockNumber(block.number);
          blockNumber$.next(block.number);
        });

        setClient(newClient);
        setConnected(true);

        (newClient as unknown as { _unsub: () => void })._unsub = unsub.unsubscribe.bind(unsub);
      } catch (err) {
        setError(err instanceof Error ? err.message : "Connection failed");
        setConnected(false);
      } finally {
        setConnecting(false);
      }
    },
    [connecting, connected]
  );

  const disconnect = useCallback(() => {
    if (client) {
      const unsub = (client as unknown as { _unsub?: () => void })._unsub;
      if (unsub) unsub();
      client.destroy();
      setClient(null);
      setConnected(false);
      setBlockNumber(0);
      blockNumber$.next(0);
    }
  }, [client]);

  useEffect(() => {
    return () => {
      if (client) {
        client.destroy();
      }
    };
  }, [client]);

  return (
    <ChainContext.Provider
      value={{
        client,
        connected,
        connecting,
        error,
        chainEndpoint,
        blockNumber,
        connect,
        disconnect,
      }}
    >
      {children}
    </ChainContext.Provider>
  );
}

export function useChain() {
  const context = useContext(ChainContext);
  if (!context) {
    throw new Error("useChain must be used within a ChainProvider");
  }
  return context;
}
