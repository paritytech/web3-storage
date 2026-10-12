// SPDX-License-Identifier: GPL-3.0-only

import { useState } from "react";
import { Archive, Plus, Server, Users } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
} from "@/components/ui/context-menu";
import { useBuckets, useSelectedBucket, selectBucket } from "@/state";
import type { BucketInfo } from "@/lib/s3-client";
import NewBucketDialog from "./NewBucketDialog";
import ManageAccessDialog from "./ManageAccessDialog";

function shortAddress(account: string): string {
  return `${account.slice(0, 6)}…${account.slice(-4)}`;
}

function providerHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** Primary-provider display parts for a bucket: address, optional host, extra count. */
function providerParts(providers: BucketInfo["providerInfo"]) {
  const first = providers[0];
  if (!first) return null;
  return {
    addr: shortAddress(first.account),
    host: first.url ? providerHost(first.url) : null,
    extra: providers.length - 1,
  };
}

/** Full provider details for the hover tooltip — one provider per line. */
function providerTitle(providers: BucketInfo["providerInfo"]): string {
  if (providers.length === 0) return "No primary provider";
  return providers
    .map((p) => `${p.account}${p.url ? ` — ${p.url}` : p.multiaddr ? ` — ${p.multiaddr}` : ""}`)
    .join("\n");
}

export default function BucketList() {
  const buckets = useBuckets();
  const selected = useSelectedBucket();
  const [showNewBucket, setShowNewBucket] = useState(false);
  const [accessBucket, setAccessBucket] = useState<BucketInfo | null>(null);

  return (
    <div className="flex flex-col gap-2" data-testid="bucket-list">
      <Button
        data-testid="new-bucket-button"
        onClick={() => setShowNewBucket(true)}
        size="sm"
        className="w-full"
      >
        <Plus className="mr-2 h-4 w-4" />
        New Bucket
      </Button>

      <div className="flex flex-col gap-1 mt-2">
        {buckets.map((bucket) => {
          const isSelected = selected?.bucketId === bucket.bucketId;

          return (
            <ContextMenu key={bucket.bucketId.toString()}>
              <ContextMenuTrigger asChild>
                <div
                  data-testid={`bucket-list-item-${bucket.bucketId}`}
                  className={`group flex items-center gap-2 rounded-lg px-3 py-2 text-sm cursor-pointer transition-colors ${
                    isSelected
                      ? "bg-primary/10 text-primary font-medium"
                      : "hover:bg-accent text-foreground"
                  }`}
                  onClick={() => selectBucket(bucket)}
                >
                  <Archive className={`h-4 w-4 flex-shrink-0 ${isSelected ? "text-primary" : "text-muted-foreground"}`} />
                  <div className="flex-1 min-w-0">
                    <p className="truncate">Bucket #{bucket.bucketId.toString()}</p>
                    {(() => {
                      const p = providerParts(bucket.providerInfo);
                      return (
                        <div
                          className="flex items-start gap-1 text-xs text-muted-foreground"
                          title={providerTitle(bucket.providerInfo)}
                          data-testid={`bucket-list-provider-${bucket.bucketId}`}
                        >
                          <Server className="h-3 w-3 flex-shrink-0 mt-0.5" />
                          {p ? (
                            <span className="flex flex-col min-w-0 leading-tight">
                              <span className="truncate">
                                {p.addr}
                                {p.extra > 0 ? ` +${p.extra}` : ""}
                              </span>
                              {p.host && <span className="truncate">{p.host}</span>}
                            </span>
                          ) : (
                            <span className="truncate">No provider</span>
                          )}
                        </div>
                      );
                    })()}
                  </div>
                  <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                    <button
                      data-testid={`bucket-list-access-${bucket.bucketId}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        setAccessBucket(bucket);
                      }}
                      className="text-muted-foreground hover:text-foreground"
                    >
                      <Users className="h-3.5 w-3.5" />
                    </button>
                  </div>
                </div>
              </ContextMenuTrigger>
              <ContextMenuContent>
                <ContextMenuItem onClick={() => setAccessBucket(bucket)}>
                  <Users className="mr-2 h-4 w-4" />
                  Manage access
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          );
        })}
      </div>

      <NewBucketDialog open={showNewBucket} onOpenChange={setShowNewBucket} />

      {accessBucket && (
        <ManageAccessDialog
          open={!!accessBucket}
          onOpenChange={() => setAccessBucket(null)}
          bucketId={accessBucket.bucketId}
          bucketName={`Bucket #${accessBucket.bucketId}`}
        />
      )}
    </div>
  );
}
