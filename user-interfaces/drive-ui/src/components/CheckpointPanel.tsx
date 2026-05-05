import { useEffect } from "react";
import { Shield, RefreshCw, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  useSelectedDrive,
  useCheckpointInfo,
  useCheckpointDuty,
  useCheckpointLoading,
  refreshCheckpoint,
  triggerCheckpoint,
  clearCheckpointState,
} from "@/state";
import { truncateHash } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";

export default function CheckpointPanel() {
  const drive = useSelectedDrive();
  const info = useCheckpointInfo();
  const duty = useCheckpointDuty();
  const loading = useCheckpointLoading();

  useEffect(() => {
    if (drive) {
      refreshCheckpoint(drive.bucketId).catch(() => { /* swallow */ });
    } else {
      clearCheckpointState();
    }
  }, [drive?.bucketId, drive]);

  if (!drive) return null;

  const handleRefresh = () => {
    refreshCheckpoint(drive.bucketId).catch(() => { /* swallow */ });
  };

  const handleTrigger = async () => {
    try {
      await triggerCheckpoint(drive.bucketId);
      toast({ title: "Checkpoint triggered", description: "Provider is creating a checkpoint..." });
    } catch (err) {
      toast({
        title: "Checkpoint failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };

  const hasCheckpoint = info && info.leafCount > 0n;

  return (
    <Card data-testid="checkpoint-panel">
      <CardContent className="p-4 space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Shield className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm font-medium">Checkpoint</span>
          </div>
          <Button
            data-testid="checkpoint-refresh"
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={handleRefresh}
            disabled={loading}
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
          </Button>
        </div>

        <div className="space-y-1 text-xs">
          <div className="flex justify-between">
            <span className="text-muted-foreground">Root CID</span>
            <span data-testid="root-cid" className="font-mono">
              {truncateHash(drive.rootCid, 8, 6)}
            </span>
          </div>
          <div className="flex justify-between">
            <span className="text-muted-foreground">Last commit</span>
            <span data-testid="last-committed-at">
              {drive.lastCommittedAt > 0 ? `block #${drive.lastCommittedAt}` : "never"}
            </span>
          </div>
          <div className="flex justify-between">
            <span className="text-muted-foreground">Strategy</span>
            <span data-testid="commit-strategy-display">
              {drive.commitStrategy.type}
              {drive.commitStrategy.type === "Batched" && ` (${drive.commitStrategy.interval})`}
            </span>
          </div>
        </div>

        <div className="border-t pt-3 space-y-1 text-xs">
          <div className="flex items-center gap-2 mb-1">
            <Shield className="h-3 w-3 text-muted-foreground" />
            <span className="font-medium text-muted-foreground">Provider checkpoint</span>
          </div>
          {hasCheckpoint ? (
            <>
              <div className="flex justify-between">
                <span className="text-muted-foreground">MMR Root</span>
                <span className="font-mono">{truncateHash(info.mmrRoot, 8, 6)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Leaf Count</span>
                <span>{info.leafCount.toString()}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Block</span>
                <span>#{info.checkpointBlock}</span>
              </div>
            </>
          ) : (
            <p className="text-muted-foreground">No checkpoint yet</p>
          )}
        </div>

        {duty && (
          <div className="flex items-center gap-2 text-xs">
            <span
              className={`h-2 w-2 rounded-full ${
                duty.ready ? "bg-green-500" : "bg-amber-500"
              }`}
            />
            <span data-testid="checkpoint-duty-status" className="text-muted-foreground">
              Provider: {duty.leafCount} leaves, {duty.ready ? "ready" : "not ready"}
            </span>
          </div>
        )}

        <Button
          data-testid="checkpoint-trigger"
          size="sm"
          className="w-full"
          onClick={handleTrigger}
          disabled={loading || (duty !== null && !duty.ready)}
        >
          {loading ? (
            <Loader2 className="mr-2 h-3 w-3 animate-spin" />
          ) : (
            <Shield className="mr-2 h-3 w-3" />
          )}
          Checkpoint Now
        </Button>
      </CardContent>
    </Card>
  );
}
