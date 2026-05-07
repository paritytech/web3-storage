import { useState } from "react";
import { Loader2, CheckCircle2, XCircle, RefreshCw, Clock, AlertTriangle } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  createDrive,
  useCreations,
  dismissCreation,
  type CreationStatus,
} from "@/state";
import { formatBytes } from "@/lib/utils";

interface NewDriveDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function formatElapsed(ms: number): string {
  const sec = Math.round(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  const rem = sec % 60;
  return `${min}m ${rem}s`;
}

function CreationStatusCard({
  item,
  onDismiss,
}: {
  item: CreationStatus;
  onDismiss: (id: string) => void;
}) {
  const isDismissible = item.stage === "ready" || item.stage === "failed";
  const showElapsed = item.stage === "waiting" || item.stage === "created";

  return (
    <div
      data-testid={`creation-card-${item.stage}`}
      className={`rounded-lg border p-3 transition-all ${
        item.stage === "ready"
          ? "border-emerald-200 bg-emerald-50"
          : item.stage === "failed"
            ? "border-red-200 bg-red-50"
            : "border-indigo-200 bg-indigo-50"
      }`}
    >
      <div className="flex items-start gap-2">
        <div className="mt-0.5">
          {item.stage === "ready" ? (
            <CheckCircle2 className="h-4 w-4 text-emerald-600" />
          ) : item.stage === "failed" ? (
            <XCircle className="h-4 w-4 text-red-600" />
          ) : (
            <RefreshCw className="h-4 w-4 animate-spin text-indigo-600" />
          )}
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <p className="text-sm font-medium truncate">{item.name}</p>
            {showElapsed && (
              <span className="flex items-center gap-1 text-xs text-muted-foreground">
                <Clock className="h-3 w-3" />
                {formatElapsed(item.elapsedMs)}
              </span>
            )}
          </div>
          <p className="text-xs text-muted-foreground mt-0.5">
            {item.stage === "submitting" && "Submitting transaction to blockchain..."}
            {item.stage === "created" && "Drive created on-chain. Setting up storage agreement..."}
            {item.stage === "waiting" && (item.statusMessage || "Waiting for provider to accept...")}
            {item.stage === "ready" && "Drive is ready to use"}
            {item.stage === "failed" && (item.error || "Something went wrong")}
          </p>
          {item.stage === "waiting" && item.elapsedMs > 60000 && (
            <div className="flex items-center gap-1 mt-1 text-xs text-amber-600">
              <AlertTriangle className="h-3 w-3" />
              <span>Taking longer than expected</span>
            </div>
          )}
        </div>
        {isDismissible && (
          <button
            data-testid={`creation-dismiss-${item.stage}`}
            onClick={() => onDismiss(item.id)}
            className="text-muted-foreground hover:text-foreground text-xs"
          >
            Dismiss
          </button>
        )}
      </div>
    </div>
  );
}

export default function NewDriveDialog({ open, onOpenChange }: NewDriveDialogProps) {
  const creations = useCreations();

  const [name, setName] = useState("");
  const [capacity, setCapacity] = useState("10000000");
  const [duration, setDuration] = useState("10000");
  const [payment, setPayment] = useState("120000000000000000");
  const [minProviders, setMinProviders] = useState("1");
  const [creating, setCreating] = useState(false);

  const handleCreate = async () => {
    setCreating(true);
    try {
      await createDrive({
        name: name || undefined,
        maxCapacity: BigInt(capacity),
        storagePeriod: parseInt(duration, 10),
        payment: BigInt(payment),
        minProviders: Math.max(1, Math.min(10, parseInt(minProviders, 10) || 1)),
      });
      setName("");
      onOpenChange(false);
    } catch {
      /* error handled in state via creations$ */
    } finally {
      setCreating(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg" data-testid="new-drive-dialog">
        <DialogHeader>
          <DialogTitle>Create New Drive</DialogTitle>
          <DialogDescription>
            Set up a new decentralized drive for your files.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium">Drive Name</label>
            <Input
              data-testid="new-drive-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="My Documents"
            />
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-1">
              <label className="text-xs font-medium">Capacity (bytes)</label>
              <Input
                data-testid="new-drive-capacity"
                type="number"
                value={capacity}
                onChange={(e) => setCapacity(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                {formatBytes(parseInt(capacity, 10) || 0)}
              </p>
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium">Duration (blocks)</label>
              <Input
                data-testid="new-drive-duration"
                type="number"
                value={duration}
                onChange={(e) => setDuration(e.target.value)}
              />
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium">Max Payment</label>
              <Input
                data-testid="new-drive-payment"
                type="number"
                value={payment}
                onChange={(e) => setPayment(e.target.value)}
              />
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium">Min Providers</label>
              <Input
                data-testid="new-drive-min-providers"
                type="number"
                min={1}
                max={10}
                value={minProviders}
                onChange={(e) => setMinProviders(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">1–10 required</p>
            </div>
          </div>

          {creations.length > 0 && (
            <div className="space-y-2">
              {creations.map((item) => (
                <CreationStatusCard
                  key={item.id}
                  item={item}
                  onDismiss={dismissCreation}
                />
              ))}
            </div>
          )}

          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button data-testid="new-drive-submit" onClick={handleCreate} disabled={creating}>
              {creating ? (
                <>
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Creating...
                </>
              ) : (
                "Create Drive"
              )}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
