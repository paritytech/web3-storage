import { CheckCircle2, XCircle, ExternalLink } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useActiveChallenge, useShowOutcomeModal, dismissChallengeResult } from "@/state/challenge.state";
import { useEndpoint } from "@/state";
import { formatTokens } from "@/lib/utils";

export default function ChallengeOutcomeDialog() {
  const showModal = useShowOutcomeModal();
  const challenge = useActiveChallenge();
  const wsEndpoint = useEndpoint();

  const explorerUrl = (blockHash: string) =>
    `https://polkadot.js.org/apps/?rpc=${encodeURIComponent(wsEndpoint)}#/explorer/query/${blockHash}`;

  if (!challenge || (challenge.status !== "defended" && challenge.status !== "slashed")) {
    return null;
  }

  const isDefended = challenge.status === "defended";
  const defense = challenge.defenseDetails;
  const slash = challenge.slashDetails;

  return (
    <Dialog open={showModal} onOpenChange={(open) => { if (!open) dismissChallengeResult(); }}>
      <DialogContent className="sm:max-w-md" data-testid="challenge-outcome-dialog">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isDefended ? (
              <>
                <CheckCircle2 className="h-5 w-5 text-emerald-500" />
                <span className="text-emerald-700 dark:text-emerald-400">Challenge Defended</span>
              </>
            ) : (
              <>
                <XCircle className="h-5 w-5 text-red-500" />
                <span className="text-red-700 dark:text-red-400">Provider Slashed!</span>
              </>
            )}
          </DialogTitle>
        </DialogHeader>

        <div className="space-y-3">
          {isDefended && defense && (
            <>
              <p className="text-sm text-muted-foreground">
                Provider responded in <span className="font-medium text-foreground">{defense.responseTimeBlocks} blocks</span>.
              </p>
              <div className="rounded-md border p-3 space-y-2 text-sm">
                <div className="grid grid-cols-2 gap-x-4 gap-y-1">
                  <span className="text-muted-foreground">Your cost</span>
                  <span className="text-right font-medium">{formatTokens(defense.challengerCost)} tokens</span>
                  <span className="text-muted-foreground">Provider cost</span>
                  <span className="text-right font-medium">{formatTokens(defense.providerCost)} tokens</span>
                </div>
                <div className="border-t pt-2 text-xs text-muted-foreground">
                  Block:{" "}
                  <a
                    href={explorerUrl(defense.blockHash)}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-emerald-700 dark:text-emerald-400 hover:underline inline-flex items-center gap-0.5"
                  >
                    #{defense.blockNumber}
                    <ExternalLink className="h-3 w-3" />
                  </a>
                </div>
              </div>
            </>
          )}

          {!isDefended && slash && (
            <>
              <p className="text-sm text-muted-foreground">
                Provider failed to respond within the deadline and was slashed.
              </p>
              <div className="rounded-md border p-3 space-y-2 text-sm">
                <div className="grid grid-cols-2 gap-x-4 gap-y-1">
                  <span className="text-muted-foreground">Slashed amount</span>
                  <span className="text-right font-medium">{formatTokens(slash.slashedAmount)} tokens</span>
                  <span className="text-muted-foreground">Your reward</span>
                  <span className="text-right font-medium">{formatTokens(slash.challengerReward)} tokens</span>
                </div>
                <div className="border-t pt-2 text-xs text-muted-foreground">
                  Block:{" "}
                  <a
                    href={explorerUrl(slash.blockHash)}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-red-700 dark:text-red-400 hover:underline inline-flex items-center gap-0.5"
                  >
                    #{slash.blockNumber}
                    <ExternalLink className="h-3 w-3" />
                  </a>
                </div>
              </div>
            </>
          )}

          {isDefended && !defense && (
            <p className="text-sm text-muted-foreground">
              Challenge was defended. Detailed information was not available (detected via polling).
            </p>
          )}

          {!isDefended && !slash && (
            <p className="text-sm text-muted-foreground">
              Provider was slashed. Detailed information was not available (detected via polling).
            </p>
          )}
        </div>

        <DialogFooter>
          <Button onClick={dismissChallengeResult}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
