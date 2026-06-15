import { useState } from "react";
import { Archive, MoreHorizontal, Plus, Trash2, Users } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from "@/components/ui/context-menu";
import { useBuckets, useSelectedBucket, selectBucket, deleteBucket } from "@/state";
import type { BucketInfo } from "@/lib/s3-client";
import { toast } from "@/components/ui/toaster";
import ConfirmDialog from "./ConfirmDialog";
import NewBucketDialog from "./NewBucketDialog";
import ManageAccessDialog from "./ManageAccessDialog";

export default function BucketList() {
  const buckets = useBuckets();
  const selected = useSelectedBucket();
  const [showNewBucket, setShowNewBucket] = useState(false);
  const [bucketToDelete, setBucketToDelete] = useState<BucketInfo | null>(null);
  const [accessBucket, setAccessBucket] = useState<BucketInfo | null>(null);

  const handleDelete = async () => {
    if (!bucketToDelete) return;
    try {
      await deleteBucket(bucketToDelete.s3BucketId);
      toast({ title: "Bucket deleted" });
    } catch (err) {
      toast({
        title: "Delete failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };

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
          const isSelected = selected?.s3BucketId === bucket.s3BucketId;
          const name = bucket.name || `Bucket ${bucket.s3BucketId}`;

          return (
            <ContextMenu key={bucket.s3BucketId.toString()}>
              <ContextMenuTrigger asChild>
                <div
                  data-testid={`bucket-list-item-${bucket.s3BucketId}`}
                  className={`group flex items-center gap-2 rounded-lg px-3 py-2 text-sm cursor-pointer transition-colors ${
                    isSelected
                      ? "bg-primary/10 text-primary font-medium"
                      : "hover:bg-accent text-foreground"
                  }`}
                  onClick={() => selectBucket(bucket)}
                >
                  <Archive className={`h-4 w-4 flex-shrink-0 ${isSelected ? "text-primary" : "text-muted-foreground"}`} />
                  <div className="flex-1 min-w-0">
                    <p className="truncate">{name}</p>
                    <p className="text-xs text-muted-foreground">
                      ID: {bucket.s3BucketId.toString()}
                    </p>
                  </div>
                  <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                    <button
                      data-testid={`bucket-list-access-${bucket.s3BucketId}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        setAccessBucket(bucket);
                      }}
                      className="text-muted-foreground hover:text-foreground"
                    >
                      <Users className="h-3.5 w-3.5" />
                    </button>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <button
                          data-testid={`bucket-list-menu-${bucket.s3BucketId}`}
                          onClick={(e) => e.stopPropagation()}
                          className="text-muted-foreground hover:text-foreground"
                        >
                          <MoreHorizontal className="h-3.5 w-3.5" />
                        </button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
                        <DropdownMenuItem
                          data-testid={`bucket-list-delete-${bucket.s3BucketId}`}
                          onClick={() => setBucketToDelete(bucket)}
                          className="text-destructive focus:text-destructive"
                        >
                          <Trash2 className="mr-2 h-4 w-4" />
                          Delete bucket
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </div>
              </ContextMenuTrigger>
              <ContextMenuContent>
                <ContextMenuItem onClick={() => setAccessBucket(bucket)}>
                  <Users className="mr-2 h-4 w-4" />
                  Manage access
                </ContextMenuItem>
                <ContextMenuSeparator />
                <ContextMenuItem
                  onClick={() => setBucketToDelete(bucket)}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="mr-2 h-4 w-4" />
                  Delete bucket
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          );
        })}
      </div>

      <NewBucketDialog open={showNewBucket} onOpenChange={setShowNewBucket} />

      <ConfirmDialog
        open={!!bucketToDelete}
        onOpenChange={() => setBucketToDelete(null)}
        title={`Delete "${bucketToDelete?.name ?? `Bucket ${bucketToDelete?.s3BucketId}`}"?`}
        description="This will permanently delete the bucket and all its objects. Storage agreements will be terminated with prorated refunds."
        onConfirm={handleDelete}
      />

      {accessBucket && (
        <ManageAccessDialog
          open={!!accessBucket}
          onOpenChange={() => setAccessBucket(null)}
          bucketId={accessBucket.layer0BucketId}
          bucketName={accessBucket.name ?? `Bucket ${accessBucket.s3BucketId}`}
        />
      )}
    </div>
  );
}
