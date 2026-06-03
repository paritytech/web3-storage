import { Outlet, NavLink } from "react-router-dom";
import {
  LayoutDashboard,
  Database,
  Search,
  Users,
  LogOut,
  Wifi,
  WifiOff,
  HardDrive,
  ArrowLeft,
} from "lucide-react";

const LANDING_URL = import.meta.env.DEV ? "http://127.0.0.1:5176/" : "../";
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
import { useNetworkSelection } from "@/state/network.state";
import { withNetworkHandoff } from "@web3-storage/network-config";
import { NetworkPicker } from "@web3-storage/network-picker";
import { Button } from "@/components/ui/button";
import { cn, truncateHash, formatTokens } from "@/lib/utils";
import ConnectDialog from "./ConnectDialog";

const navigation = [
  { name: "Dashboard", href: "/", icon: LayoutDashboard },
  { name: "Storage", href: "/storage", icon: Database },
  { name: "Explorer", href: "/explorer", icon: Search },
  { name: "Accounts", href: "/accounts", icon: Users },
];

export default function Layout() {
  const { connected, connecting, blockNumber, disconnect } = useChain();
  const { signerAddress, signerName, balance } = useStorage();
  const {
    selectedNetwork,
    networkList,
    selectNetwork,
    selectCustomNetwork,
  } = useNetworkSelection();

  return (
    <div className="flex h-screen bg-background">
      {/* Sidebar */}
      <div className="hidden w-64 flex-col border-r bg-card md:flex">
        <div className="flex h-14 items-center border-b px-4 gap-2">
          <a
            href={withNetworkHandoff(LANDING_URL, selectedNetwork)}
            data-testid="back-to-landing"
            title="Back to home"
            className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
          >
            <ArrowLeft className="h-4 w-4" />
          </a>
          <HardDrive className="h-6 w-6 text-primary" />
          <span className="font-semibold">Web3 Storage</span>
        </div>

        <nav className="flex-1 space-y-1 p-4">
          {navigation.map((item) => (
            <NavLink
              key={item.name}
              to={item.href}
              end={item.href === "/"}
              data-testid={`nav-${item.name.toLowerCase()}`}
              className={({ isActive }) =>
                cn(
                  "flex items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors",
                  isActive
                    ? "bg-primary text-primary-foreground"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground"
                )
              }
            >
              <item.icon className="h-4 w-4" />
              {item.name}
            </NavLink>
          ))}
        </nav>

        {/* Network / Block / Account — same order as drive-ui (picker stays dark here) */}
        <div className="border-t px-3 py-3 space-y-2">
          <NetworkPicker
            selectedNetwork={selectedNetwork}
            networkList={networkList}
            onSelect={(id) => { void selectNetwork(id); }}
            onSelectCustom={(input) => { void selectCustomNetwork(input); }}
          />

          <div className="flex items-center justify-between gap-2 px-2 py-1.5 text-xs">
            <div className="flex items-center gap-2 min-w-0">
              {connected ? (
                <Wifi className="h-3.5 w-3.5 text-emerald-500 flex-shrink-0" />
              ) : (
                <WifiOff className="h-3.5 w-3.5 text-muted-foreground flex-shrink-0" />
              )}
              <span className="text-muted-foreground truncate">
                {connecting
                  ? "Connecting..."
                  : connected
                    ? <span data-testid="block-number">{`Block #${blockNumber.toLocaleString()}`}</span>
                    : "Not connected"}
              </span>
            </div>
            {connected ? (
              <Button data-testid="layout-disconnect" variant="ghost" size="sm" onClick={disconnect} title="Disconnect" className="h-6 w-6 p-0">
                <LogOut className="h-3.5 w-3.5" />
              </Button>
            ) : (
              <ConnectDialog />
            )}
          </div>

          <div data-testid="account-button" className="flex items-center gap-2 w-full rounded-lg px-2 py-1.5 text-xs">
            {signerAddress ? (
              <>
                <div className="flex h-6 w-6 items-center justify-center rounded-full bg-primary/10 text-primary text-xs font-semibold flex-shrink-0">
                  {(signerName || signerAddress)[0].toUpperCase()}
                </div>
                <div className="flex flex-col items-start min-w-0">
                  <span data-testid="signer-name" className="font-medium truncate">{signerName || "Account"}</span>
                  <span data-testid="signer-address" className="text-muted-foreground font-mono truncate">
                    {truncateHash(signerAddress, 6, 4)}
                  </span>
                  {balance && (
                    <span data-testid="balance-display" className="text-muted-foreground">
                      {formatTokens(balance.free)} tokens
                    </span>
                  )}
                </div>
              </>
            ) : (
              <>
                <div className="h-6 w-6 rounded-full bg-muted flex items-center justify-center flex-shrink-0">
                  <Users className="h-3 w-3 text-muted-foreground" />
                </div>
                <span className="text-muted-foreground">No account selected</span>
              </>
            )}
          </div>
        </div>
      </div>

      {/* Main content */}
      <main className="flex-1 overflow-auto">
        <div className="container mx-auto p-6">
          <Outlet />
        </div>
      </main>
    </div>
  );
}
