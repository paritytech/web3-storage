import { useEffect } from "react";
import { Shield, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  useSelectedBucket,
  useCheckpointInfo,
  useCheckpointDuty,
  useCheckpointLoading,
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

  const bucketId = selectedBucket?.layer0BucketId ?? null;

  useEffect(() => {
    if (bucketId !== null) {
      refreshCheckpoint(bucketId).catch(() => {});
    }
  }, [bucketId]);

  const handleTrigger = async () => {
    if (bucketId === null) return;
    try {
      await triggerCheckpoint(bucketId);
      toast({ title: "Checkpoint triggered", description: "Provider will process shortly." });
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
            disabled={loading || bucketId === null}
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
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
          disabled={loading || bucketId === null}
        >
          <Shield className="mr-2 h-3.5 w-3.5" />
          Trigger Checkpoint
        </Button>
      </CardContent>
    </Card>
  );
}
