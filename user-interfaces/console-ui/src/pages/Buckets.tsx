import { useState } from "react";
import {
  Archive,
  Plus,
  File,
  RefreshCw,
  Trash2,
  ChevronRight,
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
import { toast } from "@/components/ui/toaster";

interface Bucket {
  id: string;
  name: string;
  createdAt: number;
  objectCount: number;
  totalSize: number;
}

interface S3Object {
  key: string;
  size: number;
  lastModified: number;
  etag: string;
}

export default function Buckets() {
  const { connected } = useChain();
  const [buckets, setBuckets] = useState<Bucket[]>([]);
  const [newBucketName, setNewBucketName] = useState("");
  const [creating, setCreating] = useState(false);
  const [selectedBucket, setSelectedBucket] = useState<Bucket | null>(null);
  const [objects, setObjects] = useState<S3Object[]>([]);

  const validateBucketName = (name: string): boolean => {
    // S3 bucket naming rules
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

    setCreating(true);
    try {
      // TODO: Call SDK to create bucket
      const newBucket: Bucket = {
        id: `bucket-${Date.now()}`,
        name: newBucketName,
        createdAt: Date.now(),
        objectCount: 0,
        totalSize: 0,
      };
      setBuckets([...buckets, newBucket]);
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

  const handleDeleteBucket = async (bucket: Bucket) => {
    try {
      // TODO: Call SDK to delete bucket
      setBuckets(buckets.filter((b) => b.id !== bucket.id));
      if (selectedBucket?.id === bucket.id) {
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

  const handleSelectBucket = async (bucket: Bucket) => {
    setSelectedBucket(bucket);
    // TODO: Load objects from SDK
    setObjects([]);
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

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">S3 Buckets</h1>
          <p className="text-muted-foreground">
            Manage your S3-compatible storage buckets
          </p>
        </div>
        <Button variant="outline" size="sm">
          <RefreshCw className="mr-2 h-4 w-4" />
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
        <CardContent>
          <div className="flex gap-3">
            <Input
              placeholder="my-bucket-name"
              value={newBucketName}
              onChange={(e) => setNewBucketName(e.target.value.toLowerCase())}
              className="max-w-sm"
            />
            <Button onClick={handleCreateBucket} disabled={creating}>
              {creating ? "Creating..." : "Create Bucket"}
            </Button>
          </div>
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
              key={bucket.id}
              className={`cursor-pointer transition-colors hover:border-primary ${
                selectedBucket?.id === bucket.id ? "border-primary" : ""
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
                <div className="grid grid-cols-2 gap-2 text-sm">
                  <div>
                    <p className="text-muted-foreground">Objects</p>
                    <p className="font-medium">{bucket.objectCount}</p>
                  </div>
                  <div>
                    <p className="text-muted-foreground">Size</p>
                    <p className="font-medium">
                      {bucket.totalSize === 0
                        ? "0 B"
                        : `${(bucket.totalSize / 1024).toFixed(1)} KB`}
                    </p>
                  </div>
                </div>
                <p className="mt-2 text-xs text-muted-foreground">
                  Created: {new Date(bucket.createdAt).toLocaleDateString()}
                </p>
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
            {objects.length === 0 ? (
              <div className="rounded-lg border p-4 text-center text-muted-foreground">
                <File className="mx-auto h-8 w-8 mb-2 opacity-50" />
                <p>This bucket is empty</p>
                <p className="text-sm">Upload objects to get started</p>
              </div>
            ) : (
              <div className="rounded-lg border">
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
                          {(obj.size / 1024).toFixed(1)} KB
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
