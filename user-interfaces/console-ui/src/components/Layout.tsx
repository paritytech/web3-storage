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
} from "lucide-react";
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
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

  return (
    <div className="flex h-screen bg-background">
      {/* Sidebar */}
      <div className="hidden w-64 flex-col border-r bg-card md:flex">
        <div className="flex h-14 items-center border-b px-4">
          <HardDrive className="mr-2 h-6 w-6 text-primary" />
          <span className="font-semibold">Web3 Storage</span>
        </div>

        <nav className="flex-1 space-y-1 p-4">
          {navigation.map((item) => (
            <NavLink
              key={item.name}
              to={item.href}
              end={item.href === "/"}
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

        {/* Active account info */}
        {signerAddress && (
          <div className="border-t px-4 py-3">
            <div className="flex items-center gap-2">
              <div className="flex h-8 w-8 items-center justify-center rounded-full bg-primary/10 text-primary text-sm font-medium">
                {(signerName || signerAddress)[0].toUpperCase()}
              </div>
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium truncate">{signerName || "Account"}</p>
                <p className="text-xs text-muted-foreground font-mono truncate">
                  {truncateHash(signerAddress, 6, 4)}
                </p>
              </div>
            </div>
            {balance && (
              <p className="mt-1 text-xs text-muted-foreground pl-10">
                {formatTokens(balance.free)} tokens
              </p>
            )}
          </div>
        )}

        {/* Connection status */}
        <div className="border-t p-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              {connected ? (
                <Wifi className="h-4 w-4 text-green-500" />
              ) : (
                <WifiOff className="h-4 w-4 text-muted-foreground" />
              )}
              <span className="text-sm text-muted-foreground">
                {connecting
                  ? "Connecting..."
                  : connected
                    ? `Block #${blockNumber}`
                    : "Disconnected"}
              </span>
            </div>
            {connected ? (
              <Button variant="ghost" size="sm" onClick={disconnect} title="Disconnect">
                <LogOut className="h-4 w-4" />
              </Button>
            ) : (
              <ConnectDialog />
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
