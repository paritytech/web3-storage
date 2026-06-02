import { CheckCircle2, RefreshCw, XCircle, X } from "lucide-react";
import { Button } from "@/components/ui/button";

/**
 * Stages the new create-bucket flow can be in. Bucket creation is now a
 * single atomic chain tx (negotiate happens over HTTP first, then one
 * `create_s3_bucket` extrinsic), so the only transient state is
 * `submitting`; from there it terminates as `ready` or `failed`.
 */
export type CreationStage = "submitting" | "ready" | "failed";

export interface CreationStatusItem {
  id: string;
  name: string;
  type: "bucket";
  stage: CreationStage;
  /** Elapsed milliseconds (unused now but kept so callers can keep tracking). */
  elapsedMs: number;
  error?: string;
  createdAt: number;
  /** Layer0 bucket ID populated once the tx lands. */
  bucketId?: bigint;
}

interface CreationStatusCardProps {
  item: CreationStatusItem;
  onDismiss: (id: string) => void;
  /**
   * Called when the user clicks Retry on a failed item. Only shown when
   * a retry is meaningful — e.g. the negotiation succeeded but the on-chain
   * submit failed, so the caller can re-fire the chain submit with the
   * same signed terms.
   */
  onRetry?: (id: string) => void;
}

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
        description: "Negotiating with provider and submitting to the blockchain...",
        badgeLabel: "Submitting",
      };
    case "ready":
      return {
        icon: "check",
        color: "text-green-500",
        borderColor: "border-green-500/30",
        description: "Bucket created and the storage agreement is live",
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
  onRetry,
}: CreationStatusCardProps) {
  const display = getStageDisplay(item);
  const isDismissible = item.stage === "ready" || item.stage === "failed";
  const label = "S3 Bucket";

  const badgeBg =
    item.stage === "ready"
      ? "rgb(34 197 94 / 0.1)"
      : item.stage === "failed"
        ? "rgb(239 68 68 / 0.1)"
        : "rgb(245 158 11 / 0.1)";

  return (
    <div className={`rounded-lg border ${display.borderColor} bg-card p-4 transition-all`}>
      <div className="flex items-start gap-3">
        <div className="mt-0.5">
          <StageIcon type={display.icon} color={display.color} />
        </div>
        <div className="flex-1 min-w-0">
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
          </div>

          <p className="text-sm text-muted-foreground mt-1">
            {display.description}
          </p>

          {item.stage === "failed" && onRetry && (
            <div className="mt-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => onRetry(item.id)}
              >
                <RefreshCw className="mr-2 h-3 w-3" />
                Retry on-chain submit
              </Button>
            </div>
          )}
        </div>

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