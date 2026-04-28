import { useState } from "react";
import { Outlet } from "react-router-dom";
import { HardDrive, Plug, PlugZap, User, Wallet } from "lucide-react";
import { useChain } from "@/hooks/useChain";
import { useDrive } from "@/hooks/useDrive";
import { formatTokens, truncateHash } from "@/lib/utils";
import DriveList from "./DriveList";
import ConnectDialog from "./ConnectDialog";
import AccountDialog from "./AccountDialog";

export default function Layout() {
  const { connected, blockNumber } = useChain();
  const { signerAddress, signerName, balance } = useDrive();
  const [showConnect, setShowConnect] = useState(false);
  const [showAccount, setShowAccount] = useState(false);

  return (
    <div className="flex h-screen">
      {/* Sidebar */}
      <aside className="w-64 flex-shrink-0 border-r bg-card flex flex-col">
        {/* Logo */}
        <div className="flex items-center gap-2.5 px-4 py-4 border-b">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-primary text-primary-foreground">
            <HardDrive className="h-4 w-4" />
          </div>
          <div>
            <h1 className="text-sm font-semibold">Web3 Drive</h1>
            <p className="text-xs text-muted-foreground">Decentralized Storage</p>
          </div>
        </div>

        {/* Drive list (only when signed in) */}
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

        {/* Bottom section: connection + account */}
        <div className="border-t px-3 py-3 space-y-2">
          {/* Connection status */}
          <button
            onClick={() => setShowConnect(true)}
            className="flex items-center gap-2 w-full rounded-lg px-2 py-1.5 text-xs hover:bg-accent transition-colors"
          >
            {connected ? (
              <>
                <PlugZap className="h-3.5 w-3.5 text-emerald-500" />
                <span className="text-muted-foreground">
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

          {/* Account */}
          {connected && (
            <button
              onClick={() => setShowAccount(true)}
              className="flex items-center gap-2 w-full rounded-lg px-2 py-1.5 text-xs hover:bg-accent transition-colors"
            >
              {signerAddress ? (
                <>
                  <div className="flex h-6 w-6 items-center justify-center rounded-full bg-primary/10 text-primary text-xs font-semibold">
                    {signerName ? signerName[0] : <User className="h-3 w-3" />}
                  </div>
                  <div className="flex flex-col items-start min-w-0">
                    <span className="font-medium truncate">
                      {signerName || truncateHash(signerAddress)}
                    </span>
                    {balance && (
                      <span className="flex items-center gap-1 text-muted-foreground">
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
          )}
        </div>
      </aside>

      {/* Main content */}
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
