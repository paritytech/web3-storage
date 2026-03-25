import { Shield, RefreshCw, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useDrive } from "@/hooks/useDrive";
import { truncateHash } from "@/lib/utils";

export default function CheckpointPanel() {
  const {
    checkpointInfo,
    checkpointDuty,
    checkpointLoading,
    refreshCheckpoint,
    triggerCheckpoint,
  } = useDrive();

  const hasCheckpoint = checkpointInfo && checkpointInfo.leafCount > 0n;

  return (
    <Card>
      <CardContent className="p-4 space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Shield className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm font-medium">Checkpoint</span>
          </div>
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={refreshCheckpoint}
            disabled={checkpointLoading}
          >
            <RefreshCw
              className={`h-3 w-3 ${checkpointLoading ? "animate-spin" : ""}`}
            />
          </Button>
        </div>

        {/* Last checkpoint info */}
        {hasCheckpoint ? (
          <div className="space-y-1 text-xs">
            <div className="flex justify-between">
              <span className="text-muted-foreground">MMR Root</span>
              <span className="font-mono">
                {truncateHash(checkpointInfo.mmrRoot, 8, 6)}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-muted-foreground">Leaf Count</span>
              <span>{checkpointInfo.leafCount.toString()}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-muted-foreground">Block</span>
              <span>#{checkpointInfo.checkpointBlock}</span>
            </div>
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">No checkpoint yet</p>
        )}

        {/* Provider duty status */}
        {checkpointDuty && (
          <div className="flex items-center gap-2 text-xs">
            <span
              className={`h-2 w-2 rounded-full ${
                checkpointDuty.ready ? "bg-green-500" : "bg-amber-500"
              }`}
            />
            <span className="text-muted-foreground">
              Provider: {checkpointDuty.leafCount} leaves,{" "}
              {checkpointDuty.ready ? "ready" : "not ready"}
            </span>
          </div>
        )}

        {/* Trigger button */}
        <Button
          size="sm"
          className="w-full"
          onClick={triggerCheckpoint}
          disabled={checkpointLoading || (checkpointDuty !== null && !checkpointDuty.ready)}
        >
          {checkpointLoading ? (
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
