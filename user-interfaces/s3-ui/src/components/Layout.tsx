// SPDX-License-Identifier: Apache-2.0

import { useState } from "react";
import { Outlet } from "react-router-dom";
import { ArrowLeft, Archive, Wifi, WifiOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import { NetworkPicker } from "@web3-storage/network-picker";
import BucketList from "./BucketList";
import ConnectDialog from "./ConnectDialog";
import AccountDialog from "./AccountDialog";
import {
  useIsConnected,
  useIsConnecting,
  useBlockNumber,
  useSignerAddress,
  useSignerName,
  useBalance,
  useSelectedNetworkId,
  useSelectedNetwork,
  selectNetwork,
  selectCustomNetwork,
  useNetworkList,
  getSelectedNetwork,
} from "@/state";
import { formatTokens, truncateHash } from "@/lib/utils";

export default function Layout() {
  const connected = useIsConnected();
  const connecting = useIsConnecting();
  const blockNumber = useBlockNumber();
  const signerAddress = useSignerAddress();
  const signerName = useSignerName();
  const balance = useBalance();
  const networkId = useSelectedNetworkId();
  const selectedNetwork = useSelectedNetwork();
  const networks = useNetworkList();
  const [showConnect, setShowConnect] = useState(false);
  const [showAccount, setShowAccount] = useState(false);

  const homeUrl = (() => {
    const net = getSelectedNetwork();
    const params = new URLSearchParams();
    params.set("network", networkId);
    if (networkId === "custom" && net) {
      params.set("parachainWs", net.parachainWs);
      params.set("providerHttp", net.providerHttp);
    }
    const base = import.meta.env.BASE_URL.replace(/\/(s3|s3-ui)\/?$/, "/");
    return `${base}?${params.toString()}`;
  })();

  return (
    <div className="flex h-screen">
      {/* Sidebar */}
      <aside className="w-60 border-r bg-card flex flex-col">
        <div className="p-4 border-b">
          <a
            href={homeUrl}
            className="flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground mb-3"
          >
            <ArrowLeft className="h-3 w-3" />
            Back to Home
          </a>
          <div className="flex items-center gap-2">
            <Archive className="h-5 w-5 text-primary" />
            <h1 className="font-semibold text-lg">S3 Storage</h1>
          </div>
        </div>

        <div className="p-3 border-b">
          <NetworkPicker
            selectedNetwork={selectedNetwork}
            networkList={networks}
            onSelect={(id) => { void selectNetwork(id); }}
            onSelectCustom={(input) => { void selectCustomNetwork(input); }}
            theme="light"
          />
        </div>

        <div className="flex-1 overflow-auto p-3">
          {connected && signerAddress && <BucketList />}
        </div>

        <div className="p-3 border-t space-y-2">
          {!connected ? (
            <Button
              data-testid="sidebar-connect"
              size="sm"
              className="w-full"
              onClick={() => setShowConnect(true)}
              disabled={connecting}
            >
              {connecting ? (
                <WifiOff className="mr-2 h-4 w-4 animate-pulse" />
              ) : (
                <Wifi className="mr-2 h-4 w-4" />
              )}
              {connecting ? "Connecting..." : "Connect"}
            </Button>
          ) : !signerAddress ? (
            <Button
              data-testid="sidebar-account"
              variant="outline"
              size="sm"
              className="w-full"
              onClick={() => setShowAccount(true)}
            >
              Select Account
            </Button>
          ) : (
            <button
              data-testid="sidebar-account-info"
              onClick={() => setShowAccount(true)}
              className="w-full text-left rounded-lg p-2 hover:bg-accent transition-colors"
            >
              <p className="text-sm font-medium">{signerName ?? truncateHash(signerAddress)}</p>
              <p className="text-xs text-muted-foreground">
                {balance ? formatTokens(balance.free) : "..."} tokens
              </p>
            </button>
          )}

          {connected && (
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span className="flex items-center gap-1">
                <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                Connected
              </span>
              {blockNumber !== null && <span>#{blockNumber}</span>}
            </div>
          )}
        </div>
      </aside>

      {/* Main content */}
      <main className="flex-1 overflow-auto p-6">
        <Outlet />
      </main>

      <ConnectDialog open={showConnect} onOpenChange={setShowConnect} />
      <AccountDialog open={showAccount} onOpenChange={setShowAccount} />
    </div>
  );
}
