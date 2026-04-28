import { useState, useEffect, useRef } from "react";
import { CheckCircle2, RefreshCw, XCircle, X, Clock, AlertTriangle } from "lucide-react";
import { Button } from "@/components/ui/button";

export type CreationStage =
  | "submitting"
  | "created"
  | "waiting"
  | "ready"
  | "failed";

export interface CreationStatusItem {
  id: string;
  name: string;
  type: "bucket";
  stage: CreationStage;
  /** Status message from the provider polling loop */
  statusMessage?: string;
  /** Elapsed milliseconds since the waiting stage began */
  elapsedMs: number;
  error?: string;
  createdAt: number;
  /** Layer0 bucket ID — needed for provider picker retry */
  bucketId?: bigint;
}

interface CreationStatusCardProps {
  item: CreationStatusItem;
  onDismiss: (id: string) => void;
  onChooseProvider?: (item: CreationStatusItem) => void;
}

/** Returns a human-readable elapsed time string */
function formatElapsed(ms: number): string {
  const sec = Math.round(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  const rem = sec % 60;
  return `${min}m ${rem}s`;
}

type TimingTier = "normal" | "slow" | "very_slow";

function getTimingTier(elapsedMs: number): TimingTier {
  if (elapsedMs >= 120_000) return "very_slow";
  if (elapsedMs >= 60_000) return "slow";
  return "normal";
}

const timingWarnings: Record<TimingTier, string | null> = {
  normal: null,
  slow: "Taking longer than expected — the provider may be under load",
  very_slow: "Something may be wrong — the provider could be offline or not processing agreements",
};

const timingColors: Record<TimingTier, string> = {
  normal: "text-muted-foreground",
  slow: "text-amber-500",
  very_slow: "text-red-500",
};

interface StageDisplay {
  icon: "spin" | "check" | "error";
  color: string;
  borderColor: string;
  description: string;
  badgeLabel: string;
}

function getStageDisplay(item: CreationStatusItem): StageDisplay {
  switch (item.stage) {
    case "submitting":
      return {
        icon: "spin",
        color: "text-amber-500",
        borderColor: "border-amber-500/30",
        description: "Signing transaction and submitting to the blockchain...",
        badgeLabel: "Submitting",
      };
    case "created":
      return {
        icon: "check",
        color: "text-blue-500",
        borderColor: "border-blue-500/30",
        description: "Registered on-chain. Requesting storage agreement from provider...",
        badgeLabel: "On-chain",
      };
    case "waiting":
      return {
        icon: "spin",
        color: "text-blue-500",
        borderColor: "border-blue-500/30",
        description: item.statusMessage || "Waiting for provider to accept the storage agreement...",
        badgeLabel: "Agreement pending",
      };
    case "ready":
      return {
        icon: "check",
        color: "text-green-500",
        borderColor: "border-green-500/30",
        description: "Provider accepted the agreement — storage is ready to use",
        badgeLabel: "Ready",
      };
    case "failed":
      return {
        icon: "error",
        color: "text-red-500",
        borderColor: "border-red-500/30",
        description: item.error || "An error occurred",
        badgeLabel: "Failed",
      };
  }
}

function StageIcon({ type, color }: { type: "spin" | "check" | "error"; color: string }) {
  if (type === "spin") return <RefreshCw className={`h-5 w-5 animate-spin ${color}`} />;
  if (type === "error") return <XCircle className={`h-5 w-5 ${color}`} />;
  return <CheckCircle2 className={`h-5 w-5 ${color}`} />;
}

export default function CreationStatusCard({
  item,
  onDismiss,
  onChooseProvider,
}: CreationStatusCardProps) {
  const display = getStageDisplay(item);
  const tier = getTimingTier(item.elapsedMs);
  const warning = (item.stage === "waiting" || item.stage === "created") ? timingWarnings[tier] : null;

  // Live elapsed timer — ticks every second while in active stages
  const [, setTick] = useState(0);
  const tickRef = useRef<ReturnType<typeof setInterval>>(undefined);
  useEffect(() => {
    const active = item.stage === "submitting" || item.stage === "created" || item.stage === "waiting";
    if (active) {
      tickRef.current = setInterval(() => setTick(t => t + 1), 1000);
      return () => clearInterval(tickRef.current);
    }
    clearInterval(tickRef.current);
  }, [item.stage]);

  const showElapsed = item.stage === "waiting" || item.stage === "created";
  const isDismissible = item.stage === "ready" || item.stage === "failed";
  const label = "S3 Bucket";

  const badgeBg =
    item.stage === "ready"
      ? "rgb(34 197 94 / 0.1)"
      : item.stage === "failed"
        ? "rgb(239 68 68 / 0.1)"
        : item.stage === "submitting"
          ? "rgb(245 158 11 / 0.1)"
          : "rgb(59 130 246 / 0.1)";

  return (
    <div className={`rounded-lg border ${display.borderColor} bg-card p-4 transition-all`}>
      <div className="flex items-start gap-3">
        <div className="mt-0.5">
          <StageIcon type={display.icon} color={display.color} />
        </div>
        <div className="flex-1 min-w-0">
          {/* Header: name + badge */}
          <div className="flex items-center gap-2">
            <p className="text-sm font-medium truncate">
              {label}: {item.name}
            </p>
            <span
              className={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${display.color}`}
              style={{ backgroundColor: badgeBg }}
            >
              {display.badgeLabel}
            </span>
            {showElapsed && (
              <span className={`inline-flex items-center gap-1 text-xs ${timingColors[tier]}`}>
                <Clock className="h-3 w-3" />
                {formatElapsed(item.elapsedMs)}
              </span>
            )}
          </div>

          {/* Status description */}
          <p className="text-sm text-muted-foreground mt-1">
            {display.description}
          </p>

          {/* Timing warning — shows alongside status when things are slow */}
          {warning && (
            <div className={`flex items-center gap-1.5 mt-2 text-xs ${timingColors[tier]}`}>
              <AlertTriangle className="h-3.5 w-3.5 flex-shrink-0" />
              <span>{warning}</span>
            </div>
          )}

          {/* Action buttons for failed stage */}
          {item.stage === "failed" && onChooseProvider && item.bucketId && (
            <div className="mt-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => onChooseProvider(item)}
              >
                Choose Provider
              </Button>
            </div>
          )}
        </div>

        {/* Dismiss button — only for terminal states */}
        {isDismissible && (
          <button
            onClick={() => onDismiss(item.id)}
            className="text-muted-foreground hover:text-foreground transition-colors"
          >
            <X className="h-4 w-4" />
          </button>
        )}
      </div>
    </div>
  );
}
