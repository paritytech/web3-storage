// SPDX-License-Identifier: GPL-3.0-only

import { useEffect } from "react";
import { Shield, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  useSelectedDrive,
  useCheckpointInfo,
  useCheckpointLoading,
  refreshCheckpoint,
  clearCheckpointState,
} from "@/state";
import { truncateHash } from "@/lib/utils";

export default function CheckpointPanel() {
  const drive = useSelectedDrive();
  const info = useCheckpointInfo();
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
      </CardContent>
    </Card>
  );
}
