import { useState, useEffect } from "react";
import { AlertCircle, Database, Users } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
import { formatTokens, formatBytes } from "@/lib/utils";
import S3Tab from "@/components/S3Tab";
import BucketInfoPanel from "@/components/BucketInfoPanel";
import CheckpointPanel from "@/components/CheckpointPanel";

export default function Storage() {
  const { connected } = useChain();
  const { signerAddress, balance, providerCapacity, listAccessibleBucketIds, buckets } = useStorage();
  const [selectedLayer0BucketId, setSelectedLayer0BucketId] = useState<bigint | null>(null);
  const [sharedBucketIds, setSharedBucketIds] = useState<bigint[]>([]);

  // Fetch shared bucket IDs (buckets where user is a member but not owner)
  useEffect(() => {
    if (!signerAddress) {
      setSharedBucketIds([]);
      return;
    }
    listAccessibleBucketIds().then((ids) => {
      // Filter out buckets the user owns (those are already shown in the main list)
      const ownedIds = new Set(buckets.map((b) => b.id));
      setSharedBucketIds(ids.filter((id) => !ownedIds.has(id)));
    }).catch(() => setSharedBucketIds([]));
  }, [signerAddress, buckets, listAccessibleBucketIds]);

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Storage</h1>
          <p className="text-muted-foreground">Manage your files and objects</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <Database className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p className="text-muted-foreground">Connect to the network to manage storage</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (!signerAddress) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Storage</h1>
          <p className="text-muted-foreground">Manage your files and objects</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <AlertCircle className="mx-auto h-12 w-12 mb-4 text-yellow-500" />
            <p className="text-muted-foreground mb-2">No signer set</p>
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
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Storage</h1>
        <p className="text-muted-foreground">Manage your files and objects</p>
      </div>

      {/* Info bar */}
      <div className="flex flex-wrap items-center gap-4 rounded-lg border bg-card p-3 text-sm">
        {balance && (
          <div className="flex items-center gap-2">
            <span className="text-muted-foreground">Balance:</span>
            <span className="font-medium">{formatTokens(balance.free)} tokens</span>
          </div>
        )}
        {providerCapacity && (
          <div className="flex items-center gap-2 flex-1 min-w-[200px]">
            <span className="text-muted-foreground">Provider capacity:</span>
            <div className="flex items-center gap-2 flex-1 max-w-xs">
              <div className="h-2 flex-1 rounded-full bg-secondary">
                <div
                  className="h-full rounded-full bg-primary transition-all"
                  style={{
                    width: `${providerCapacity.max > 0n ? Number((providerCapacity.used * 100n) / providerCapacity.max) : 0}%`,
                  }}
                />
              </div>
              <span className="text-xs text-muted-foreground whitespace-nowrap">
                {formatBytes(Number(providerCapacity.used))} / {formatBytes(Number(providerCapacity.max))}
              </span>
            </div>
          </div>
        )}
      </div>

      {/* S3 Storage */}
      <S3Tab onBucketSelect={setSelectedLayer0BucketId} />

      {/* Shared with me */}
      {sharedBucketIds.length > 0 && (
        <Card>
          <CardContent className="py-4">
            <div className="flex items-center gap-2 mb-3">
              <Users className="h-4 w-4 text-muted-foreground" />
              <h3 className="font-semibold text-sm">Shared with me</h3>
              <span className="text-xs text-muted-foreground">({sharedBucketIds.length})</span>
            </div>
            <div className="flex flex-wrap gap-2">
              {sharedBucketIds.map((id) => (
                <button
                  key={id.toString()}
                  className={`px-3 py-1.5 text-sm rounded-md border transition-colors ${
                    selectedLayer0BucketId === id
                      ? "bg-primary text-primary-foreground border-primary"
                      : "bg-card hover:bg-accent border-border"
                  }`}
                  onClick={() => setSelectedLayer0BucketId(id)}
                >
                  Bucket #{id.toString()}
                </button>
              ))}
            </div>
          </CardContent>
        </Card>
      )}

      {selectedLayer0BucketId && (
        <>
          <BucketInfoPanel bucketId={selectedLayer0BucketId} />
          <CheckpointPanel bucketId={selectedLayer0BucketId} />
        </>
      )}
    </div>
  );
}
