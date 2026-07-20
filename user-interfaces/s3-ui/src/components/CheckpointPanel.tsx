// SPDX-License-Identifier: GPL-3.0-only

import { useEffect, useState } from "react";
import { Shield, RefreshCw, Loader2, CheckCircle2, Swords, History } from "lucide-react";
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
  useOpenChallenges,
  useOpenChallengesLoading,
  refreshOpenChallenges,
  useAnchorBlock,
} from "@/state";
import { useActiveChallenge, useChallengeStatus, useChallengeHistory } from "@/state/challenge.state";
import { truncateHash } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";
import { getS3Client } from "@/state";
import ChallengeDialog from "./ChallengeDialog";
import ChallengeOutcomeDialog from "./ChallengeOutcomeDialog";

interface CheckpointPanelProps {
  onShowHistory?: () => void;
}

export default function CheckpointPanel({ onShowHistory }: CheckpointPanelProps) {
  const selectedBucket = useSelectedBucket();
  const info = useCheckpointInfo();
  const duty = useCheckpointDuty();
  const loading = useCheckpointLoading();
  const status = useCheckpointStatus();
  const challengeStatus = useChallengeStatus();
  const activeChallenge = useActiveChallenge();
  const openChallenges = useOpenChallenges();
  const challengesLoading = useOpenChallengesLoading();
  // Challenge deadlines are on the pallet's anchor clock.
  const anchorBlock = useAnchorBlock();

  const allHistory = useChallengeHistory();

  const [challengeOpen, setChallengeOpen] = useState(false);
  const [providers, setProviders] = useState<string[]>([]);

  const bucketId = selectedBucket?.layer0BucketId ?? null;
  const busy = status === "triggering" || status === "polling";
  const challengeBusy = challengeStatus !== "idle";

  const history = bucketId !== null
    ? allHistory.filter((e) => e.bucketId === bucketId.toString())
    : allHistory;

  useEffect(() => {
    if (bucketId !== null) {
      refreshCheckpoint(bucketId).catch(() => {});
      refreshOpenChallenges(bucketId).catch(() => {});
      const client = getS3Client();
      if (client.hasApi()) {
        client.getBucketProviders(bucketId).then(setProviders).catch(() => setProviders([]));
      }
    } else {
      setProviders([]);
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
            onClick={() => {
              if (bucketId !== null) {
                refreshCheckpoint(bucketId);
                refreshOpenChallenges(bucketId);
              }
            }}
            disabled={loading || busy || bucketId === null}
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading || challengesLoading ? "animate-spin" : ""}`} />
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {/* Checkpoint progress status */}
        {status === "triggering" && (
          <div className="flex items-center gap-2 rounded-md bg-blue-500/15 px-3 py-2 text-sm text-blue-600 dark:text-blue-400 font-medium">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Sending checkpoint trigger...
          </div>
        )}
        {status === "polling" && (
          <div className="flex items-center gap-2 rounded-md bg-orange-500/15 px-3 py-2 text-sm text-orange-600 dark:text-orange-400 font-medium">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Waiting for on-chain confirmation...
          </div>
        )}
        {status === "confirmed" && (
          <div className="flex items-center gap-2 rounded-md bg-emerald-500/15 px-3 py-2 text-sm text-emerald-600 dark:text-emerald-400 font-medium">
            <CheckCircle2 className="h-3.5 w-3.5" />
            Checkpoint confirmed on-chain
          </div>
        )}

        {/* Challenge submission status */}
        {challengeStatus === "submitting" && (
          <div className="flex items-center gap-2 rounded-md bg-blue-500/15 px-3 py-2 text-sm text-blue-600 dark:text-blue-400 font-medium">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Submitting challenge...
          </div>
        )}
        {challengeStatus === "submitted" && (
          <div className="flex items-center gap-2 rounded-md bg-orange-500/15 px-3 py-2 text-sm text-orange-600 dark:text-orange-400 font-medium">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Watching for response...
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

        {/* Open challenges list */}
        {openChallenges.length > 0 && (
          <div className="space-y-2">
            <p className="text-xs font-medium text-muted-foreground uppercase tracking-wide">
              Open Challenges ({openChallenges.length})
            </p>
            <div className="space-y-1.5">
              {openChallenges.map((c) => {
                const blocksLeft = anchorBlock ? c.deadline - anchorBlock : null;
                const isExpired = blocksLeft !== null && blocksLeft <= 0;
                const isWatching =
                  activeChallenge?.status === "submitted" &&
                  activeChallenge.challengeId.deadline === c.deadline &&
                  activeChallenge.challengeId.index === c.index;
                return (
                  <div
                    key={`${c.deadline}-${c.index}`}
                    className={`rounded-md border px-3 py-2 text-xs space-y-1 ${
                      isWatching
                        ? "border-blue-200 bg-blue-500/5"
                        : isExpired
                          ? "border-red-200 bg-red-500/5"
                          : "border-orange-200 bg-orange-500/5"
                    }`}
                  >
                    <div className="flex items-center justify-between">
                      <span className="flex items-center gap-1.5 font-medium">
                        <Swords className="h-3 w-3" />
                        Leaf {c.leafIndex.toString()}, Chunk {c.chunkIndex.toString()}
                      </span>
                      {isWatching ? (
                        <span className="flex items-center gap-1 text-blue-600 dark:text-blue-400">
                          <Loader2 className="h-3 w-3 animate-spin" />
                          Watching...
                        </span>
                      ) : isExpired ? (
                        <span className="text-red-600 dark:text-red-400 font-medium">Expired</span>
                      ) : blocksLeft !== null ? (
                        <span className="text-orange-600 dark:text-orange-400">
                          {blocksLeft} blocks left
                        </span>
                      ) : null}
                    </div>
                    <div className="flex justify-between text-muted-foreground">
                      <span>Provider: {truncateHash(c.provider, 6, 4)}</span>
                      <span>Deadline: #{c.deadline}</span>
                    </div>
                    <div className="text-muted-foreground">
                      Challenger: {truncateHash(c.challenger, 6, 4)}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        <div className="flex gap-2">
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

          <Button
            data-testid="challenge-provider"
            variant="outline"
            size="sm"
            onClick={() => setChallengeOpen(true)}
            disabled={!info || challengeBusy || bucketId === null}
          >
            <Swords className="mr-2 h-3.5 w-3.5" />
            Challenge Provider
          </Button>

          <Button
            data-testid="challenge-history"
            variant="outline"
            size="sm"
            onClick={onShowHistory}
            disabled={history.length === 0 || !onShowHistory}
          >
            <History className="mr-2 h-3.5 w-3.5" />
            History ({history.length})
          </Button>
        </div>

        <ChallengeDialog
          open={challengeOpen}
          onOpenChange={setChallengeOpen}
          providers={providers}
        />
        <ChallengeOutcomeDialog />
      </CardContent>
    </Card>
  );
}
