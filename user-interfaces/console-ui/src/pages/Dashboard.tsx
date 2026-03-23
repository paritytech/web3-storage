import {
  Archive,
  FileUp,
  FileDown,
  Activity,
  Wifi,
  WifiOff,
} from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useChain } from "@/hooks/useChain";

export default function Dashboard() {
  const { connected, blockNumber, chainEndpoint } = useChain();

  const stats = [
    {
      title: "S3 Buckets",
      value: "0",
      description: "S3-compatible buckets",
      icon: Archive,
    },
    {
      title: "Uploads",
      value: "0",
      description: "Total files uploaded",
      icon: FileUp,
    },
    {
      title: "Downloads",
      value: "0",
      description: "Total files downloaded",
      icon: FileDown,
    },
  ];

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Dashboard</h1>
        <p className="text-muted-foreground">
          Overview of your Web3 Storage usage
        </p>
      </div>

      {/* Connection Status Card */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center gap-2">
            {connected ? (
              <Wifi className="h-5 w-5 text-green-500" />
            ) : (
              <WifiOff className="h-5 w-5 text-muted-foreground" />
            )}
            <CardTitle>Network Status</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {connected ? (
            <div className="grid gap-4 md:grid-cols-2">
              <div>
                <p className="text-sm text-muted-foreground">Chain Endpoint</p>
                <p className="font-mono text-sm">{chainEndpoint}</p>
              </div>
              <div>
                <p className="text-sm text-muted-foreground">Latest Block</p>
                <p className="font-mono text-sm">#{blockNumber}</p>
              </div>
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">
              Connect to the network to view storage statistics and manage your
              files.
            </p>
          )}
        </CardContent>
      </Card>

      {/* Stats Grid */}
      <div className="grid gap-4 md:grid-cols-3">
        {stats.map((stat) => (
          <Card key={stat.title}>
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <CardTitle className="text-sm font-medium">{stat.title}</CardTitle>
              <stat.icon className="h-4 w-4 text-muted-foreground" />
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold">{stat.value}</div>
              <p className="text-xs text-muted-foreground">{stat.description}</p>
            </CardContent>
          </Card>
        ))}
      </div>

      {/* Recent Activity */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Activity className="h-5 w-5" />
            <CardTitle>Recent Activity</CardTitle>
          </div>
          <CardDescription>Your latest storage operations</CardDescription>
        </CardHeader>
        <CardContent>
          <div className="text-center py-8 text-muted-foreground">
            <Activity className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p>No recent activity</p>
            <p className="text-sm">
              Start by creating a bucket or uploading files
            </p>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
