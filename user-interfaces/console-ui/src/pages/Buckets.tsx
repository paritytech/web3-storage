import { useState, useEffect } from "react";
import {
  Archive,
  Plus,
  File,
  RefreshCw,
  Trash2,
  ChevronRight,
  AlertCircle,
} from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
import { toast } from "@/components/ui/toaster";
import { formatBytes } from "@/lib/utils";
import type { BucketInfo, S3ObjectInfo } from "@/lib/storage";

export default function Buckets() {
  const { connected } = useChain();
  const {
    signerAddress,
    buckets: chainBuckets,
    loading,
    createBucket,
    refreshBuckets,
    deleteBucket: sdkDeleteBucket,
    listObjects,
  } = useStorage();

  const [buckets, setBuckets] = useState<BucketInfo[]>([]);
  const [newBucketName, setNewBucketName] = useState("");
  const [capacity, setCapacity] = useState("1000000000"); // 1 GB default
  const [duration, setDuration] = useState("10000");
  const [maxPayment, setMaxPayment] = useState("1000000000000000"); // 1000 tokens
  const [creating, setCreating] = useState(false);
  const [selectedBucket, setSelectedBucket] = useState<BucketInfo | null>(null);
  const [objects, setObjects] = useState<S3ObjectInfo[]>([]);
  const [loadingObjects, setLoadingObjects] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);

  // Sync chain buckets to local state
  useEffect(() => {
    if (chainBuckets.length > 0) {
      setBuckets(chainBuckets);
    }
  }, [chainBuckets]);

  // Refresh buckets when signer is set
  useEffect(() => {
    if (signerAddress && connected) {
      refreshBuckets();
    }
  }, [signerAddress, connected, refreshBuckets]);

  const validateBucketName = (name: string): boolean => {
    if (name.length < 3 || name.length > 63) return false;
    if (!/^[a-z0-9]/.test(name)) return false;
    if (!/[a-z0-9]$/.test(name)) return false;
    if (!/^[a-z0-9.-]+$/.test(name)) return false;
    if (/\.\./.test(name)) return false;
    return true;
  };

  const handleCreateBucket = async () => {
    if (!newBucketName.trim()) {
      toast({ title: "Error", description: "Bucket name is required", variant: "destructive" });
      return;
    }

    if (!validateBucketName(newBucketName)) {
      toast({
        title: "Error",
        description: "Invalid bucket name. Must be 3-63 characters, lowercase, and follow S3 naming rules.",
        variant: "destructive",
      });
      return;
    }

    if (!signerAddress) {
      toast({
        title: "Error",
        description: "Please set a signer in the Accounts page first",
        variant: "destructive",
      });
      return;
    }

    setCreating(true);
    try {
      await createBucket(newBucketName, {
        capacity: BigInt(capacity),
        duration: parseInt(duration, 10),
        maxPayment: BigInt(maxPayment),
      });

      setNewBucketName("");
      toast({ title: "Success", description: `Bucket "${newBucketName}" created` });
    } catch (err) {
      toast({
        title: "Error",
        description: err instanceof Error ? err.message : "Failed to create bucket",
        variant: "destructive",
      });
    } finally {
      setCreating(false);
    }
  };

  const handleDeleteBucket = async (bucket: BucketInfo) => {
    try {
      await sdkDeleteBucket(bucket.name);
      setBuckets(buckets.filter((b) => b.s3BucketId !== bucket.s3BucketId));
      if (selectedBucket?.s3BucketId === bucket.s3BucketId) {
        setSelectedBucket(null);
        setObjects([]);
      }
      toast({ title: "Success", description: `Bucket "${bucket.name}" deleted` });
    } catch (err) {
      toast({
        title: "Error",
        description: err instanceof Error ? err.message : "Failed to delete bucket",
        variant: "destructive",
      });
    }
  };

  const handleSelectBucket = async (bucket: BucketInfo) => {
    setSelectedBucket(bucket);
    setLoadingObjects(true);
    try {
      const objs = await listObjects(bucket.layer0BucketId);
      setObjects(objs);
    } catch (err) {
      console.error("Failed to list objects:", err);
      setObjects([]);
    } finally {
      setLoadingObjects(false);
    }
  };

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">S3 Buckets</h1>
          <p className="text-muted-foreground">Manage your S3-compatible storage buckets</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <Archive className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p className="text-muted-foreground">
              Connect to the network to manage S3 buckets
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (!signerAddress) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">S3 Buckets</h1>
          <p className="text-muted-foreground">Manage your S3-compatible storage buckets</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <AlertCircle className="mx-auto h-12 w-12 mb-4 text-yellow-500" />
            <p className="text-muted-foreground mb-2">
              No signer set
            </p>
            <p className="text-sm text-muted-foreground">
              Go to the Accounts page to set a signing account first
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">S3 Buckets</h1>
          <p className="text-muted-foreground">
            Manage your S3-compatible storage buckets
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={refreshBuckets} disabled={loading}>
          <RefreshCw className={`mr-2 h-4 w-4 ${loading ? "animate-spin" : ""}`} />
          Refresh
        </Button>
      </div>

      {/* Create Bucket */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Plus className="h-5 w-5" />
            Create New Bucket
          </CardTitle>
          <CardDescription>
            Create a new S3-compatible bucket. Names must be 3-63 characters,
            lowercase, and follow S3 naming rules.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex gap-3">
            <Input
              placeholder="my-bucket-name"
              value={newBucketName}
              onChange={(e) => setNewBucketName(e.target.value.toLowerCase())}
              className="max-w-sm"
            />
            <Button onClick={handleCreateBucket} disabled={creating || loading}>
              {creating ? "Creating..." : "Create Bucket"}
            </Button>
          </div>

          <Button
            variant="ghost"
            size="sm"
            onClick={() => setShowAdvanced(!showAdvanced)}
          >
            {showAdvanced ? "Hide" : "Show"} Advanced Options
          </Button>

          {showAdvanced && (
            <div className="grid gap-4 md:grid-cols-3 pt-2">
              <div className="space-y-2">
                <label className="text-sm font-medium">Capacity (bytes)</label>
                <Input
                  type="number"
                  value={capacity}
                  onChange={(e) => setCapacity(e.target.value)}
                  placeholder="1000000000"
                />
                <p className="text-xs text-muted-foreground">
                  {formatBytes(parseInt(capacity, 10) || 0)}
                </p>
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Duration (blocks)</label>
                <Input
                  type="number"
                  value={duration}
                  onChange={(e) => setDuration(e.target.value)}
                  placeholder="10000"
                />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Max Payment (12 decimals)</label>
                <Input
                  type="number"
                  value={maxPayment}
                  onChange={(e) => setMaxPayment(e.target.value)}
                  placeholder="1000000000000000"
                />
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      {/* Buckets List */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {buckets.length === 0 ? (
          <Card className="col-span-full">
            <CardContent className="py-8 text-center">
              <Archive className="mx-auto h-12 w-12 mb-4 opacity-50" />
              <p className="text-muted-foreground">No buckets yet</p>
              <p className="text-sm text-muted-foreground">
                Create your first S3 bucket to start storing objects
              </p>
            </CardContent>
          </Card>
        ) : (
          buckets.map((bucket) => (
            <Card
              key={bucket.s3BucketId.toString()}
              className={`cursor-pointer transition-colors hover:border-primary ${
                selectedBucket?.s3BucketId === bucket.s3BucketId ? "border-primary" : ""
              }`}
              onClick={() => handleSelectBucket(bucket)}
            >
              <CardHeader className="pb-2">
                <div className="flex items-center justify-between">
                  <CardTitle className="flex items-center gap-2 text-lg">
                    <Archive className="h-5 w-5 text-primary" />
                    {bucket.name}
                  </CardTitle>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDeleteBucket(bucket);
                    }}
                  >
                    <Trash2 className="h-4 w-4 text-muted-foreground hover:text-destructive" />
                  </Button>
                </div>
              </CardHeader>
              <CardContent>
                <div className="text-xs text-muted-foreground">
                  <p>S3 ID: {bucket.s3BucketId.toString()}</p>
                  <p>Layer0 ID: {bucket.layer0BucketId.toString()}</p>
                </div>
              </CardContent>
            </Card>
          ))
        )}
      </div>

      {/* Selected Bucket Objects */}
      {selectedBucket && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Archive className="h-5 w-5" />
              {selectedBucket.name}
              <ChevronRight className="h-4 w-4" />
              <span className="text-muted-foreground">Objects</span>
            </CardTitle>
            <CardDescription>Browse objects in this bucket</CardDescription>
          </CardHeader>
          <CardContent>
            {loadingObjects ? (
              <div className="rounded-lg border p-4 text-center text-muted-foreground">
                <RefreshCw className="mx-auto h-8 w-8 mb-2 animate-spin opacity-50" />
                <p>Loading objects...</p>
              </div>
            ) : objects.length === 0 ? (
              <div className="rounded-lg border p-4 text-center text-muted-foreground">
                <File className="mx-auto h-8 w-8 mb-2 opacity-50" />
                <p>This bucket is empty</p>
                <p className="text-sm">Go to Upload page to add objects</p>
              </div>
            ) : (
              <div className="rounded-lg border overflow-x-auto">
                <table className="w-full">
                  <thead>
                    <tr className="border-b bg-muted/50">
                      <th className="px-4 py-2 text-left text-sm font-medium">Key</th>
                      <th className="px-4 py-2 text-left text-sm font-medium">Size</th>
                      <th className="px-4 py-2 text-left text-sm font-medium">Last Modified</th>
                      <th className="px-4 py-2 text-left text-sm font-medium">ETag</th>
                    </tr>
                  </thead>
                  <tbody>
                    {objects.map((obj) => (
                      <tr key={obj.key} className="border-b">
                        <td className="px-4 py-2 font-mono text-sm">{obj.key}</td>
                        <td className="px-4 py-2 text-sm">
                          {formatBytes(obj.size)}
                        </td>
                        <td className="px-4 py-2 text-sm">
                          {new Date(obj.lastModified).toLocaleString()}
                        </td>
                        <td className="px-4 py-2 font-mono text-xs text-muted-foreground">
                          {obj.etag.slice(0, 8)}...
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
