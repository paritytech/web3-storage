import { useState } from "react";
import { Outlet } from "react-router-dom";
import { HardDrive, Plug, PlugZap, User, Wallet, ArrowLeft } from "lucide-react";

const LANDING_URL = import.meta.env.DEV ? "http://127.0.0.1:5176/" : "../";
import {
  useIsConnected,
  useBlockNumber,
  useSignerAddress,
  useSignerName,
  useBalance,
} from "@/state";
import {
  useSelectedNetwork,
  useNetworkList,
  selectNetwork,
  selectCustomNetwork,
} from "@/state/network.state";
import { withNetworkHandoff } from "@web3-storage/network-config";
import { NetworkPicker } from "@web3-storage/network-picker";
import { formatTokens, truncateHash } from "@/lib/utils";
import DriveList from "./DriveList";
import ConnectDialog from "./ConnectDialog";
import AccountDialog from "./AccountDialog";

export default function Layout() {
  const connected = useIsConnected();
  const blockNumber = useBlockNumber();
  const signerAddress = useSignerAddress();
  const signerName = useSignerName();
  const balance = useBalance();
  const selectedNetwork = useSelectedNetwork();
  const networkList = useNetworkList();
  const [showConnect, setShowConnect] = useState(false);
  const [showAccount, setShowAccount] = useState(false);

  return (
    <div className="flex h-screen">
      <aside className="w-64 flex-shrink-0 border-r bg-card flex flex-col">
        <div className="flex items-center gap-2.5 px-4 py-4 border-b">
          <a
            href={withNetworkHandoff(LANDING_URL, selectedNetwork)}
            data-testid="back-to-landing"
            title="Back to home"
            className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground transition-colors"
          >
            <ArrowLeft className="h-4 w-4" />
          </a>
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-primary text-primary-foreground">
            <HardDrive className="h-4 w-4" />
          </div>
          <div>
            <h1 className="text-sm font-semibold">Web3 Drive</h1>
            <p className="text-xs text-muted-foreground">Decentralized Storage</p>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto px-3 py-3">
          {connected && signerAddress ? (
            <DriveList />
          ) : (
            <div className="flex flex-col items-center justify-center h-full text-center text-muted-foreground">
              <HardDrive className="h-8 w-8 mb-2 opacity-50" />
              <p className="text-sm">
                {!connected
                  ? "Connect to chain to get started"
                  : "Select an account to view drives"}
              </p>
            </div>
          )}
        </div>

        <div
          className="border-t px-3 py-3 space-y-2"
          style={{
            ['--np-bg' as string]: '#ffffff',
            ['--np-fg' as string]: '#1f2937',
            ['--np-muted' as string]: '#6b7280',
            ['--np-border' as string]: '#e5e7eb',
            ['--np-input-bg' as string]: '#f9fafb',
            ['--np-accent' as string]: '#7c3aed',
          }}
        >
          <NetworkPicker
            selectedNetwork={selectedNetwork}
            networkList={networkList}
            onSelect={(id) => { void selectNetwork(id); }}
            onSelectCustom={(input) => { void selectCustomNetwork(input); }}
          />

          <button
            data-testid="connect-button"
            onClick={() => setShowConnect(true)}
            className="flex items-center gap-2 w-full rounded-lg px-2 py-1.5 text-xs hover:bg-accent transition-colors"
          >
            {connected ? (
              <>
                <PlugZap className="h-3.5 w-3.5 text-emerald-500" />
                <span data-testid="block-number" className="text-muted-foreground">
                  Block #{blockNumber.toLocaleString()}
                </span>
              </>
            ) : (
              <>
                <Plug className="h-3.5 w-3.5 text-muted-foreground" />
                <span className="text-muted-foreground">Not connected</span>
              </>
            )}
          </button>

          <button
            data-testid="account-button"
            onClick={() => setShowAccount(true)}
            className="flex items-center gap-2 w-full rounded-lg px-2 py-1.5 text-xs hover:bg-accent transition-colors"
          >
            {connected && signerAddress ? (
              <>
                <div className="flex h-6 w-6 items-center justify-center rounded-full bg-primary/10 text-primary text-xs font-semibold">
                  {signerName ? signerName[0] : <User className="h-3 w-3" />}
                </div>
                <div className="flex flex-col items-start min-w-0">
                  <span data-testid="signer-address" className="font-medium truncate">
                    {signerName || truncateHash(signerAddress)}
                  </span>
                  {balance && (
                    <span data-testid="balance-display" className="flex items-center gap-1 text-muted-foreground">
                      <Wallet className="h-3 w-3" />
                      {formatTokens(balance.free)} tokens
                    </span>
                  )}
                </div>
              </>
            ) : (
              <>
                <User className="h-3.5 w-3.5 text-muted-foreground" />
                <span className="text-muted-foreground">Select account</span>
              </>
            )}
          </button>
        </div>
      </aside>

      <main className="flex-1 flex flex-col overflow-hidden">
        <div className="flex-1 overflow-y-auto p-6">
          <Outlet />
        </div>
      </main>

      <ConnectDialog open={showConnect} onOpenChange={setShowConnect} />
      <AccountDialog open={showAccount} onOpenChange={setShowAccount} />
    </div>
  );
}
