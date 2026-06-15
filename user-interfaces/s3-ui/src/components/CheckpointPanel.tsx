import { useEffect } from "react";
import { Shield, RefreshCw, Loader2, CheckCircle2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  useSelectedBucket,
  useCheckpointInfo,
  useCheckpointDuty,
  useCheckpointLoading,
  useCheckpointStatus,
  refreshCheckpoint,
  triggerCheckpoint,
} from "@/state";
import { truncateHash } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";

export default function CheckpointPanel() {
  const selectedBucket = useSelectedBucket();
  const info = useCheckpointInfo();
  const duty = useCheckpointDuty();
  const loading = useCheckpointLoading();
  const status = useCheckpointStatus();

  const bucketId = selectedBucket?.layer0BucketId ?? null;
  const busy = status === "triggering" || status === "polling";

  useEffect(() => {
    if (bucketId !== null) {
      refreshCheckpoint(bucketId).catch(() => {});
    }
  }, [bucketId]);

  const handleTrigger = async () => {
    if (bucketId === null) return;
    try {
      await triggerCheckpoint(bucketId);
    } catch (err) {
      toast({
        title: "Checkpoint failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };

  return (
    <Card data-testid="checkpoint-panel">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <CardTitle className="text-sm flex items-center gap-2">
            <Shield className="h-4 w-4" />
            Checkpoint Status
          </CardTitle>
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            onClick={() => bucketId !== null && refreshCheckpoint(bucketId)}
            disabled={loading || busy || bucketId === null}
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {/* Progress status card */}
        {status === "triggering" && (
          <div className="flex items-center gap-2 rounded-md bg-blue-500/10 px-3 py-2 text-sm text-blue-600 dark:text-blue-400">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Sending checkpoint trigger...
          </div>
        )}
        {status === "polling" && (
          <div className="flex items-center gap-2 rounded-md bg-amber-500/10 px-3 py-2 text-sm text-amber-600 dark:text-amber-400">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Waiting for on-chain confirmation...
          </div>
        )}
        {status === "confirmed" && (
          <div className="flex items-center gap-2 rounded-md bg-emerald-500/10 px-3 py-2 text-sm text-emerald-600 dark:text-emerald-400">
            <CheckCircle2 className="h-3.5 w-3.5" />
            Checkpoint confirmed on-chain
          </div>
        )}

        {info ? (
          <div className="grid grid-cols-2 gap-2 text-sm">
            <div>
              <p className="text-xs text-muted-foreground">MMR Root</p>
              <p className="font-mono text-xs">{truncateHash(info.mmrRoot, 8, 6)}</p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground">Leaf Count</p>
              <p>{info.leafCount.toString()}</p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground">Checkpoint Block</p>
              <p>#{info.checkpointBlock}</p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground">Start Seq</p>
              <p>{info.startSeq.toString()}</p>
            </div>
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">No checkpoint data yet.</p>
        )}

        {duty && (
          <div className="flex items-center gap-2 text-sm">
            <span
              className={`h-2 w-2 rounded-full ${duty.ready ? "bg-emerald-500" : "bg-amber-500"}`}
            />
            <span className="text-muted-foreground">
              Provider duty: {duty.ready ? "Ready" : "Pending"}
            </span>
          </div>
        )}

        <Button
          data-testid="trigger-checkpoint"
          variant="outline"
          size="sm"
          onClick={handleTrigger}
          disabled={loading || busy || bucketId === null}
        >
          {busy ? (
            <Loader2 className="mr-2 h-3.5 w-3.5 animate-spin" />
          ) : (
            <Shield className="mr-2 h-3.5 w-3.5" />
          )}
          {busy ? "Processing..." : "Trigger Checkpoint"}
        </Button>
      </CardContent>
    </Card>
  );
}
