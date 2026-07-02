// SPDX-License-Identifier: Apache-2.0

import { useEffect, useState } from "react";
import { Dices } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useCheckpointInfo, useSelectedBucket, getS3Client } from "@/state";
import { submitChallenge } from "@/state/challenge.state";
import { truncateHash } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";

interface ChallengeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providers: string[];
}

export default function ChallengeDialog({ open, onOpenChange, providers }: ChallengeDialogProps) {
  const selectedBucket = useSelectedBucket();
  const info = useCheckpointInfo();

  const [selectedProvider, setSelectedProvider] = useState<string>("");
  const [leafIndex, setLeafIndex] = useState("0");
  const [chunkIndex, setChunkIndex] = useState("0");
  const [chunkCount, setChunkCount] = useState<number | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const bucketId = selectedBucket?.layer0BucketId ?? null;
  const leafCount = info?.leafCount ?? 0n;
  const maxLeaf = leafCount > 0n ? leafCount - 1n : 0n;

  const leafVal = BigInt(leafIndex || "0");
  const isLeafValid = leafCount > 0n && leafVal >= 0n && leafVal <= maxLeaf;

  // Auto-select first provider when list changes
  useEffect(() => {
    if (providers.length > 0 && !providers.includes(selectedProvider)) {
      setSelectedProvider(providers[0]!);
    }
  }, [providers, selectedProvider]);

  // Fetch chunk count when leaf index changes
  useEffect(() => {
    setChunkCount(null);
    if (bucketId === null || !isLeafValid) return;
    let cancelled = false;
    const client = getS3Client();
    client
      .getLeafChunkCount(bucketId, leafVal)
      .then((count) => {
        if (!cancelled) setChunkCount(count);
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [bucketId, leafIndex, isLeafValid]); // eslint-disable-line react-hooks/exhaustive-deps

  const randomizeLeaf = () => {
    if (leafCount <= 0n) return;
    const max = Number(maxLeaf);
    const random = Math.floor(Math.random() * (max + 1));
    setLeafIndex(random.toString());
  };

  const handleSubmit = async () => {
    if (bucketId === null || !isLeafValid || !selectedProvider) return;
    setSubmitting(true);
    try {
      await submitChallenge(bucketId, selectedProvider, BigInt(leafIndex), BigInt(chunkIndex || "0"));
      onOpenChange(false);
      setLeafIndex("0");
      setChunkIndex("0");
      setChunkCount(null);
    } catch (err) {
      toast({
        title: "Challenge failed",
        description: err instanceof Error ? err.message : "Error submitting challenge",
        variant: "destructive",
      });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md" data-testid="challenge-dialog">
        <DialogHeader>
          <DialogTitle>Challenge Provider</DialogTitle>
          <DialogDescription>
            Submit an on-chain challenge to verify a provider still holds your data.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {info && (
            <div className="rounded-md border p-3 space-y-1 text-sm">
              <div className="flex justify-between">
                <span className="text-muted-foreground">MMR Root</span>
                <span className="font-mono text-xs">{truncateHash(info.mmrRoot, 8, 6)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Leaf Count</span>
                <span>{info.leafCount.toString()}</span>
              </div>
            </div>
          )}

          {/* Provider selection */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Provider</label>
            {providers.length <= 1 ? (
              <p className="font-mono text-xs text-muted-foreground">
                {providers[0] ? truncateHash(providers[0], 8, 6) : "No providers"}
              </p>
            ) : (
              <select
                data-testid="challenge-provider-select"
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs font-mono transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                value={selectedProvider}
                onChange={(e) => setSelectedProvider(e.target.value)}
              >
                {providers.map((addr) => (
                  <option key={addr} value={addr}>
                    {truncateHash(addr, 10, 8)}
                  </option>
                ))}
              </select>
            )}
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium">Leaf Index</label>
            <div className="flex gap-2">
              <Input
                data-testid="challenge-leaf-index"
                type="number"
                min="0"
                max={maxLeaf.toString()}
                value={leafIndex}
                onChange={(e) => setLeafIndex(e.target.value)}
                placeholder="0"
              />
              <Button
                variant="outline"
                size="icon"
                onClick={randomizeLeaf}
                disabled={leafCount <= 0n}
                title="Pick random leaf"
              >
                <Dices className="h-4 w-4" />
              </Button>
            </div>
            {leafCount > 0n && (
              <p className="text-xs text-muted-foreground">
                Valid range: 0 to {maxLeaf.toString()}
              </p>
            )}
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium">Chunk Index</label>
            <Input
              data-testid="challenge-chunk-index"
              type="number"
              min="0"
              max={chunkCount !== null ? (chunkCount - 1).toString() : undefined}
              value={chunkIndex}
              onChange={(e) => setChunkIndex(e.target.value)}
              placeholder="0"
            />
            <p className="text-xs text-muted-foreground">
              {chunkCount !== null
                ? `This leaf has ${chunkCount} chunk${chunkCount !== 1 ? "s" : ""} (valid: 0 to ${chunkCount - 1}).`
                : "Index of the chunk within the leaf."}
            </p>
          </div>

          <div className="rounded-md bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-400">
            A deposit will be reserved to cover challenge costs. If the provider fails to respond,
            your deposit and tx fees are refunded from the slash. If the provider responds, the cost
            is split based on response speed (you pay the majority).
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            data-testid="submit-challenge"
            onClick={handleSubmit}
            disabled={submitting || bucketId === null || !isLeafValid || !selectedProvider}
          >
            {submitting ? "Submitting..." : "Submit Challenge"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
