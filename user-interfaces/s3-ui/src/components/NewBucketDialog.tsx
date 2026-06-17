// SPDX-License-Identifier: GPL-3.0-only

import { useState } from "react";
import { CheckCircle2, XCircle, RefreshCw } from "lucide-react";
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
  canRetryCreation,
  createBucket,
  dismissCreation,
  getSignerAddress,
  retryCreation,
  useCreations,
  type CreationStatus,
} from "@/state";
import {
  negotiateTerms,
  parseMultiaddrToHttp,
  type AvailableProvider,
  type SignedTerms,
} from "@/lib/s3-client";
import { formatBytes } from "@/lib/utils";
import ProviderPickerPanel from "./ProviderPickerPanel";

interface NewBucketDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function CreationStatusCard({
  item,
  onDismiss,
  onRetry,
}: {
  item: CreationStatus;
  onDismiss: (id: string) => void;
  onRetry?: (id: string) => void;
}) {
  const isDismissible = item.stage === "ready" || item.stage === "failed";

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
          <p className="text-sm font-medium truncate">{item.name}</p>
          <p className="text-xs text-muted-foreground mt-0.5">
            {item.stage === "submitting" && "Submitting on-chain..."}
            {item.stage === "ready" && "Bucket is ready to use"}
            {item.stage === "failed" && (item.error || "Something went wrong")}
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

export default function NewBucketDialog({ open, onOpenChange }: NewBucketDialogProps) {
  const creations = useCreations();

  const [name, setName] = useState("");
  const [capacity, setCapacity] = useState("10485760");
  const [duration, setDuration] = useState("10000");
  const [pricePerByte, setPricePerByte] = useState("0");
  const [submitting, setSubmitting] = useState(false);
  const [negotiateError, setNegotiateError] = useState<string | null>(null);
  const [isShowProviderPicker, setIsShowProviderPicker] = useState<boolean>(false);

  const handleProviderSelect = async (provider: AvailableProvider) => {
    setSubmitting(true);
    setNegotiateError(null);
    try {
      const url = parseMultiaddrToHttp(provider.multiaddr);
      if (!url) {
        setNegotiateError(
          `Provider ${provider.account} has an unparseable multiaddr: ${provider.multiaddr}`,
        );
        return;
      }

      const owner = getSignerAddress();
      if (!owner) {
        setNegotiateError("Signer not set");
        return;
      }

      let signed: SignedTerms;
      try {
        signed = await negotiateTerms(url, {
          owner,
          max_bytes: BigInt(capacity),
          duration: parseInt(duration, 10),
          price_per_byte: BigInt(pricePerByte || "0"),
          replica_params: null,
          bucket_id: null,
        });
      } catch (err) {
        setNegotiateError(
          err instanceof Error ? err.message : "Failed to negotiate with provider",
        );
        return;
      }

      const bucket = await createBucket({
        name: name || "Untitled Bucket",
        provider,
        url,
        signed,
      });
      if (bucket) {
        setName("");
        onOpenChange(false);
      }
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-3xl" data-testid="new-bucket-dialog">
        <DialogHeader>
          <DialogTitle>Create New Bucket</DialogTitle>
          <DialogDescription>
            Set up a new S3 bucket with a decentralized storage provider.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium">Bucket Name</label>
            <Input
              data-testid="new-bucket-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="my-bucket"
            />
          </div>

          <div className="grid gap-3 sm:grid-cols-3">
            <div className="space-y-1">
              <label className="text-xs font-medium">Capacity (bytes)</label>
              <Input
                data-testid="new-bucket-capacity"
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
                data-testid="new-bucket-duration"
                type="number"
                value={duration}
                onChange={(e) => setDuration(e.target.value)}
              />
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium">Price per byte (per block)</label>
              <Input
                data-testid="new-bucket-price"
                type="number"
                min="0"
                value={pricePerByte}
                onChange={(e) => setPricePerByte(e.target.value)}
              />
            </div>
          </div>

          {isShowProviderPicker ? (
            <ProviderPickerPanel
              onSelect={handleProviderSelect}
              requiredCapacity={BigInt(capacity || "0")}
              requiredDuration={parseInt(duration, 10) || 0}
              requiredPricePerByte={BigInt(pricePerByte || "0")}
              disabled={submitting}
            />
          ) : (
            <Button data-testid="find-matching-providers" onClick={() => setIsShowProviderPicker(true)}>
              Find matching Providers
            </Button>
          )}

          {negotiateError && (
            <p data-testid="negotiate-error" className="text-sm text-red-600">
              {negotiateError}
            </p>
          )}

          {creations.length > 0 && (
            <div className="space-y-2">
              {creations.map((item) => (
                <CreationStatusCard
                  key={item.id}
                  item={item}
                  onDismiss={dismissCreation}
                  onRetry={
                    canRetryCreation(item.id)
                      ? (id) => {
                          void retryCreation(id);
                        }
                      : undefined
                  }
                />
              ))}
            </div>
          )}

          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
