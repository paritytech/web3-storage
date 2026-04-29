import { useState, useEffect, useCallback } from "react";
import {
  ShieldCheck,
  ChevronDown,
  ChevronRight,
  RefreshCw,
  Settings,
  Coins,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent } from "@/components/ui/card";
import { useStorage } from "@/hooks/useStorage";
import { useChain } from "@/hooks/useChain";
import { toast } from "@/components/ui/toaster";
import { formatTokens, truncateHash } from "@/lib/utils";
import type { CheckpointStatus } from "@/lib/storage";

interface CheckpointPanelProps {
  bucketId: bigint;
}

const UNIT = 1_000_000_000_000n; // 12 decimals

export default function CheckpointPanel({ bucketId }: CheckpointPanelProps) {
  const { blockNumber } = useChain();
  const {
    fetchCheckpointStatus,
    submitCheckpoint,
    configureCheckpointWindow,
    fundCheckpointPool,
    claimCheckpointRewards,
  } = useStorage();

  const [expanded, setExpanded] = useState(false);
  const [status, setStatus] = useState<CheckpointStatus | null>(null);
  const [loadingStatus, setLoadingStatus] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  // Fund pool state
  const [fundAmount, setFundAmount] = useState("");
  const [funding, setFunding] = useState(false);

  // Claim state
  const [claiming, setClaiming] = useState(false);

  // Config state
  const [showConfig, setShowConfig] = useState(false);
  const [cfgInterval, setCfgInterval] = useState("");
  const [cfgGracePeriod, setCfgGracePeriod] = useState("");
  const [cfgEnabled, setCfgEnabled] = useState(true);
  const [configuring, setConfiguring] = useState(false);

  const refresh = useCallback(async () => {
    setLoadingStatus(true);
    try {
      const s = await fetchCheckpointStatus(bucketId);
      setStatus(s);
      if (s) {
        setCfgInterval(String(s.config.interval));
        setCfgGracePeriod(String(s.config.gracePeriod));
        setCfgEnabled(s.config.enabled);
      }
    } finally {
      setLoadingStatus(false);
    }
  }, [bucketId, fetchCheckpointStatus]);

  // Fetch on mount and bucketId change
  useEffect(() => {
    refresh();
  }, [refresh]);

  // Re-fetch periodically (every 5 blocks) to stay current
  useEffect(() => {
    if (blockNumber > 0 && blockNumber % 5 === 0) {
      refresh();
    }
  }, [blockNumber, refresh]);

  const handleSubmitCheckpoint = async () => {
    setSubmitting(true);
    try {
      await submitCheckpoint(bucketId);
      toast({ title: "Checkpoint submitted", description: `Window ${status?.currentWindow?.toString() ?? "?"}` });
      await refresh();
    } catch (err) {
      toast({ title: "Checkpoint failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    } finally {
      setSubmitting(false);
    }
  };

  const handleFundPool = async () => {
    if (!fundAmount.trim()) return;
    setFunding(true);
    try {
      const amount = BigInt(Math.floor(parseFloat(fundAmount) * Number(UNIT)));
      await fundCheckpointPool(bucketId, amount);
      toast({ title: "Pool funded", description: `${fundAmount} tokens` });
      setFundAmount("");
      await refresh();
    } catch (err) {
      toast({ title: "Fund failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    } finally {
      setFunding(false);
    }
  };

  const handleClaimRewards = async () => {
    setClaiming(true);
    try {
      await claimCheckpointRewards(bucketId);
      toast({ title: "Rewards claimed" });
      await refresh();
    } catch (err) {
      toast({ title: "Claim failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    } finally {
      setClaiming(false);
    }
  };

  const handleConfigureCheckpoint = async () => {
    setConfiguring(true);
    try {
      await configureCheckpointWindow(
        bucketId,
        parseInt(cfgInterval, 10),
        parseInt(cfgGracePeriod, 10),
        cfgEnabled,
      );
      toast({ title: "Checkpoint config updated" });
      setShowConfig(false);
      await refresh();
    } catch (err) {
      toast({ title: "Config failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    } finally {
      setConfiguring(false);
    }
  };

  const canSubmit = status &&
    status.config.enabled &&
    (status.lastWindow < status.currentWindow) &&
    status.currentWindow > 0n;

  return (
    <Card>
      {/* Header — always visible */}
      <button
        className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-muted/30 transition-colors"
        onClick={() => setExpanded(!expanded)}
      >
        <ShieldCheck className="h-5 w-5 text-primary" />
        <span className="font-medium text-sm">Checkpoints</span>

        {status && (
          <>
            <span
              className={`ml-1 rounded-full px-2 py-0.5 text-xs font-medium ${
                status.config.enabled
                  ? "bg-green-100 text-green-700 dark:bg-green-900 dark:text-green-300"
                  : "bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400"
              }`}
            >
              {status.config.enabled ? "Enabled" : "Disabled"}
            </span>
            <span className="ml-auto text-xs text-muted-foreground">
              Window {status.currentWindow.toString()}
              {status.lastWindow > 0n && ` · Last: ${status.lastWindow.toString()}`}
            </span>
          </>
        )}

        {loadingStatus && <RefreshCw className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />}

        {expanded ? (
          <ChevronDown className="h-4 w-4 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-4 w-4 text-muted-foreground" />
        )}
      </button>

      {/* Expanded content */}
      {expanded && status && (
        <CardContent className="border-t pt-4 space-y-4">
          {/* Grid: Status + Rewards + Config */}
          <div className="grid gap-4 md:grid-cols-3">

            {/* a. Status & Submit */}
            <div className="space-y-3">
              <h4 className="text-xs font-semibold uppercase text-muted-foreground">Status</h4>

              {status.snapshot ? (
                <div className="space-y-1 text-sm">
                  <div className="flex justify-between">
                    <span className="text-muted-foreground">MMR Root</span>
                    <span className="font-mono text-xs">{truncateHash(status.snapshot.mmrRoot, 8, 6)}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-muted-foreground">Leaf Count</span>
                    <span>{status.snapshot.leafCount.toString()}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-muted-foreground">Last Block</span>
                    <span>#{status.snapshot.checkpointBlock}</span>
                  </div>
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">No checkpoint snapshot yet</p>
              )}

              <div className="flex justify-between text-sm">
                <span className="text-muted-foreground">Current Window</span>
                <span className="font-medium">{status.currentWindow.toString()}</span>
              </div>
              <div className="flex justify-between text-sm">
                <span className="text-muted-foreground">Last Window</span>
                <span>{status.lastWindow.toString()}</span>
              </div>

              <Button
                size="sm"
                className="w-full"
                disabled={!canSubmit || submitting}
                onClick={handleSubmitCheckpoint}
              >
                {submitting ? (
                  <>
                    <RefreshCw className="mr-2 h-3.5 w-3.5 animate-spin" />
                    Submitting...
                  </>
                ) : (
                  "Submit Checkpoint"
                )}
              </Button>
              {!canSubmit && status.config.enabled && status.currentWindow > 0n && (
                <p className="text-xs text-muted-foreground text-center">
                  Already checkpointed for current window
                </p>
              )}
            </div>

            {/* b. Reward Pool */}
            <div className="space-y-3">
              <h4 className="text-xs font-semibold uppercase text-muted-foreground flex items-center gap-1">
                <Coins className="h-3.5 w-3.5" />
                Reward Pool
              </h4>

              <div className="flex justify-between text-sm">
                <span className="text-muted-foreground">Pool Balance</span>
                <span className="font-medium">{formatTokens(status.poolBalance)} tokens</span>
              </div>

              <div className="flex items-center gap-2">
                <Input
                  type="number"
                  placeholder="Amount (tokens)"
                  value={fundAmount}
                  onChange={(e) => setFundAmount(e.target.value)}
                  className="text-sm"
                />
                <Button
                  size="sm"
                  variant="outline"
                  onClick={handleFundPool}
                  disabled={funding || !fundAmount.trim()}
                >
                  {funding ? "..." : "Fund"}
                </Button>
              </div>

              <div className="flex justify-between text-sm">
                <span className="text-muted-foreground">Pending Rewards</span>
                <span className="font-medium">{formatTokens(status.pendingRewards)} tokens</span>
              </div>

              <Button
                size="sm"
                variant="outline"
                className="w-full"
                disabled={claiming || status.pendingRewards === 0n}
                onClick={handleClaimRewards}
              >
                {claiming ? "Claiming..." : "Claim Rewards"}
              </Button>
            </div>

            {/* c. Configuration */}
            <div className="space-y-3">
              <button
                className="flex items-center gap-1 text-xs font-semibold uppercase text-muted-foreground hover:text-foreground"
                onClick={() => setShowConfig(!showConfig)}
              >
                <Settings className="h-3.5 w-3.5" />
                Configuration
                {showConfig ? (
                  <ChevronDown className="h-3 w-3" />
                ) : (
                  <ChevronRight className="h-3 w-3" />
                )}
              </button>

              <div className="flex justify-between text-sm">
                <span className="text-muted-foreground">Interval</span>
                <span>{status.config.interval} blocks</span>
              </div>
              <div className="flex justify-between text-sm">
                <span className="text-muted-foreground">Grace Period</span>
                <span>{status.config.gracePeriod} blocks</span>
              </div>

              {showConfig && (
                <div className="space-y-2 rounded-md border p-3">
                  <div className="space-y-1">
                    <label className="text-xs font-medium">Interval (blocks)</label>
                    <Input
                      type="number"
                      value={cfgInterval}
                      onChange={(e) => setCfgInterval(e.target.value)}
                      className="text-sm"
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-xs font-medium">Grace Period (blocks)</label>
                    <Input
                      type="number"
                      value={cfgGracePeriod}
                      onChange={(e) => setCfgGracePeriod(e.target.value)}
                      className="text-sm"
                    />
                  </div>
                  <label className="flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      checked={cfgEnabled}
                      onChange={(e) => setCfgEnabled(e.target.checked)}
                      className="rounded"
                    />
                    Enabled
                  </label>
                  <Button
                    size="sm"
                    className="w-full"
                    onClick={handleConfigureCheckpoint}
                    disabled={configuring}
                  >
                    {configuring ? "Updating..." : "Update Config"}
                  </Button>
                </div>
              )}
            </div>
          </div>
        </CardContent>
      )}
    </Card>
  );
}
