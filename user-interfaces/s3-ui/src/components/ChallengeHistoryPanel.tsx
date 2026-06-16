import { useEffect } from "react";
import { CheckCircle2, XCircle, ExternalLink, RefreshCw, ArrowLeft, Swords, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useChallengeHistory, useChallengeHistoryLoading, refreshChallengeHistory } from "@/state/challenge.state";
import type { ChallengeHistoryEntry } from "@/state/challenge.state";
import { useEndpoint } from "@/state";
import { truncateHash, formatTokens } from "@/lib/utils";

interface ChallengeHistoryPanelProps {
  bucketId: bigint | null;
  onClose: () => void;
}

function HistoryEntry({ entry, explorerUrl }: { entry: ChallengeHistoryEntry; explorerUrl: (hash: string) => string }) {
  const isDefended = entry.status === "defended";

  return (
    <div className={`rounded-lg border px-4 py-3 text-sm space-y-2 ${
      isDefended ? "border-emerald-200 bg-emerald-500/5" : "border-red-200 bg-red-500/5"
    }`}>
      <div className="flex items-center justify-between">
        <span className={`flex items-center gap-2 font-medium ${
          isDefended ? "text-emerald-700 dark:text-emerald-400" : "text-red-700 dark:text-red-400"
        }`}>
          {isDefended ? <CheckCircle2 className="h-4 w-4" /> : <XCircle className="h-4 w-4" />}
          {isDefended ? "Defended" : "Slashed"}
        </span>
        <a
          href={explorerUrl(entry.blockHash)}
          target="_blank"
          rel="noopener noreferrer"
          className={`text-xs hover:underline inline-flex items-center gap-0.5 ${
            isDefended ? "text-emerald-700 dark:text-emerald-400" : "text-red-700 dark:text-red-400"
          }`}
        >
          Block #{entry.blockNumber}
          <ExternalLink className="h-3 w-3" />
        </a>
      </div>

      <div className="grid grid-cols-2 gap-x-6 gap-y-1 text-xs">
        <span className="text-muted-foreground">
          Deadline <span className="text-foreground font-mono">#{entry.challengeId.deadline}</span>
        </span>
        <span className="text-muted-foreground">
          Index <span className="text-foreground font-mono">{entry.challengeId.index}</span>
        </span>
        {entry.provider && (
          <span className="text-muted-foreground col-span-2">
            Provider: <span className="text-foreground font-mono">{truncateHash(entry.provider, 8, 6)}</span>
          </span>
        )}
      </div>

      {isDefended && entry.defenseDetails && (
        <div className="grid grid-cols-2 gap-x-6 text-xs">
          <span className="text-muted-foreground">
            Response: <span className="text-foreground">{entry.defenseDetails.responseTimeBlocks} blocks</span>
          </span>
          <span className="text-muted-foreground">
            Your cost: <span className="text-foreground">{formatTokens(entry.defenseDetails.challengerCost)} tokens</span>
          </span>
        </div>
      )}

      {!isDefended && entry.slashDetails && (
        <div className="grid grid-cols-2 gap-x-6 text-xs">
          <span className="text-muted-foreground">
            Slashed: <span className="text-foreground">{formatTokens(entry.slashDetails.slashedAmount)} tokens</span>
          </span>
          <span className="text-muted-foreground">
            Your reward: <span className="text-foreground">{formatTokens(entry.slashDetails.challengerReward)} tokens</span>
          </span>
        </div>
      )}
    </div>
  );
}

export default function ChallengeHistoryPanel({ bucketId, onClose }: ChallengeHistoryPanelProps) {
  const allHistory = useChallengeHistory();
  const historyLoading = useChallengeHistoryLoading();
  const wsEndpoint = useEndpoint();

  const history = bucketId !== null
    ? allHistory.filter((e) => e.bucketId === bucketId.toString())
    : allHistory;

  const explorerUrl = (blockHash: string) =>
    `https://polkadot.js.org/apps/?rpc=${encodeURIComponent(wsEndpoint)}#/explorer/query/${blockHash}`;

  // Fetch from chain when panel opens
  useEffect(() => {
    refreshChallengeHistory(bucketId).catch(() => {});
  }, [bucketId]);

  return (
    <div className="flex flex-col h-full" data-testid="challenge-history-panel">
      {/* Header */}
      <div className="flex items-center justify-between pb-4 border-b">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="sm" onClick={onClose} className="gap-1.5">
            <ArrowLeft className="h-4 w-4" />
            Back to objects
          </Button>
          <div className="w-px h-5 bg-border" />
          <h2 className="text-sm font-medium flex items-center gap-2">
            <Swords className="h-4 w-4" />
            Challenge History
            {!historyLoading && (
              <span className="text-muted-foreground font-normal">({history.length})</span>
            )}
          </h2>
        </div>

        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          onClick={() => refreshChallengeHistory(bucketId).catch(() => {})}
          disabled={historyLoading}
        >
          <RefreshCw className={`h-3.5 w-3.5 ${historyLoading ? "animate-spin" : ""}`} />
        </Button>
      </div>

      {/* List */}
      <div className="flex-1 overflow-y-auto pt-4 space-y-3">
        {historyLoading && history.length === 0 ? (
          <div className="flex items-center justify-center py-16">
            <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
          </div>
        ) : history.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 text-muted-foreground">
            <Swords className="h-10 w-10 mb-3 opacity-30" />
            <p className="text-sm">No challenge history for this bucket.</p>
            <p className="text-xs mt-1">Challenge outcomes will appear here after resolution.</p>
          </div>
        ) : (
          history.map((entry, i) => (
            <HistoryEntry
              key={`${entry.challengeId.deadline}-${entry.challengeId.index}-${i}`}
              entry={entry}
              explorerUrl={explorerUrl}
            />
          ))
        )}
      </div>
    </div>
  );
}
