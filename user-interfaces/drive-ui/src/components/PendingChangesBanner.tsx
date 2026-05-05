import { useMemo, useState } from "react";
import { GitBranch, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useSelectedDrive, useBlockNumber, commitChanges } from "@/state";
import { toast } from "@/components/ui/toaster";
import { truncateHash } from "@/lib/utils";

export default function PendingChangesBanner() {
  const drive = useSelectedDrive();
  const block = useBlockNumber();
  const [committing, setCommitting] = useState(false);

  const pendingInfo = useMemo(() => {
    if (!drive) return null;
    const hasPending = drive.pendingRootCid !== null;
    const strategy = drive.commitStrategy;

    if (!hasPending) {
      // For Manual strategy show "no pending changes" hint? Skip — only show banner when actionable.
      return null;
    }

    if (strategy.type === "Immediate") {
      // Immediate shouldn't have pending typically; treat as transient.
      return { kind: "transient" as const };
    }

    if (strategy.type === "Batched") {
      const interval = strategy.interval;
      const lastCommit = drive.lastCommittedAt || 0;
      const blocksSince = block && lastCommit ? block - lastCommit : 0;
      const remaining = interval > blocksSince ? interval - blocksSince : 0;
      return {
        kind: "batched" as const,
        remaining,
        interval,
      };
    }

    if (strategy.type === "Manual") {
      return { kind: "manual" as const };
    }

    return null;
  }, [drive, block]);

  if (!drive || !pendingInfo) return null;

  const handleCommit = async () => {
    if (!drive) return;
    setCommitting(true);
    try {
      await commitChanges(drive.driveId);
      toast({ title: "Changes committed" });
    } catch (err) {
      toast({
        title: "Commit failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    } finally {
      setCommitting(false);
    }
  };

  return (
    <div
      data-testid="pending-changes-banner"
      className="mt-3 flex items-center justify-between gap-3 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs"
    >
      <div className="flex items-center gap-2 min-w-0">
        <GitBranch className="h-3.5 w-3.5 text-amber-600 flex-shrink-0" />
        <div className="min-w-0">
          <span className="font-medium text-amber-900">Pending changes</span>
          <span className="text-amber-700 ml-1">
            {pendingInfo.kind === "manual" &&
              "this drive uses the Manual commit strategy — commit to update the on-chain root CID."}
            {pendingInfo.kind === "batched" &&
              (pendingInfo.remaining > 0
                ? `next auto-commit in ~${pendingInfo.remaining} blocks (interval ${pendingInfo.interval}).`
                : "auto-commit pending in the next batch.")}
            {pendingInfo.kind === "transient" && "syncing on-chain root..."}
          </span>
          {drive.pendingRootCid && (
            <span className="text-amber-700 ml-1 font-mono">
              ({truncateHash(drive.pendingRootCid, 6, 4)})
            </span>
          )}
        </div>
      </div>
      {pendingInfo.kind === "manual" && (
        <Button
          data-testid="commit-now-button"
          size="sm"
          variant="outline"
          className="h-7 text-xs"
          disabled={committing}
          onClick={handleCommit}
        >
          {committing ? (
            <>
              <Loader2 className="mr-1.5 h-3 w-3 animate-spin" />
              Committing...
            </>
          ) : (
            "Commit Now"
          )}
        </Button>
      )}
    </div>
  );
}
