import { Outlet, NavLink } from "react-router-dom";
import {
  LayoutDashboard,
  HardDrive,
  Archive,
  Upload,
  Download,
  Search,
  Users,
  Settings,
  Wifi,
  WifiOff,
} from "lucide-react";
import { useChain } from "@/hooks/useChain";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import ConnectDialog from "./ConnectDialog";

const navigation = [
  { name: "Dashboard", href: "/", icon: LayoutDashboard },
  { name: "Drives", href: "/drives", icon: HardDrive },
  { name: "S3 Buckets", href: "/buckets", icon: Archive },
  { name: "Upload", href: "/upload", icon: Upload },
  { name: "Download", href: "/download", icon: Download },
  { name: "Explorer", href: "/explorer", icon: Search },
  { name: "Accounts", href: "/accounts", icon: Users },
];

export default function Layout() {
  const { connected, connecting, blockNumber, disconnect } = useChain();

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
              <Button variant="ghost" size="sm" onClick={disconnect}>
                <Settings className="h-4 w-4" />
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
