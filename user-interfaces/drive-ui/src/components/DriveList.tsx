// SPDX-License-Identifier: GPL-3.0-only

import { useState } from "react";
import { HardDrive, MoreHorizontal, Plus, Trash2, Users } from "lucide-react";
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
import { useDrives, useSelectedDrive, selectDrive, deleteDrive } from "@/state";
import { formatBytes } from "@/lib/utils";
import type { DriveInfo } from "@/lib/drive-client";
import { toast } from "@/components/ui/toaster";
import ConfirmDialog from "./ConfirmDialog";
import NewDriveDialog from "./NewDriveDialog";
import ManageAccessDialog from "./ManageAccessDialog";

export default function DriveList() {
  const drives = useDrives();
  const selected = useSelectedDrive();
  const [showNewDrive, setShowNewDrive] = useState(false);
  const [driveToDelete, setDriveToDelete] = useState<DriveInfo | null>(null);
  const [accessDrive, setAccessDrive] = useState<DriveInfo | null>(null);

  const handleDelete = async () => {
    if (!driveToDelete) return;
    try {
      await deleteDrive(driveToDelete.driveId);
      toast({ title: "Drive deleted" });
    } catch (err) {
      toast({
        title: "Delete failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  };

  return (
    <div className="flex flex-col gap-2" data-testid="drive-list">
      <Button
        data-testid="new-drive-button"
        onClick={() => setShowNewDrive(true)}
        size="sm"
        className="w-full"
      >
        <Plus className="mr-2 h-4 w-4" />
        New Drive
      </Button>

      <div className="flex flex-col gap-1 mt-2">
        {drives.map((drive) => {
          const isSelected = selected?.driveId === drive.driveId;
          const name = drive.name || `Drive ${drive.driveId}`;

          return (
            <ContextMenu key={drive.driveId.toString()}>
              <ContextMenuTrigger asChild>
                <div
                  data-testid={`drive-list-item-${drive.driveId}`}
                  className={`group flex items-center gap-2 rounded-lg px-3 py-2 text-sm cursor-pointer transition-colors ${
                    isSelected
                      ? "bg-primary/10 text-primary font-medium"
                      : "hover:bg-accent text-foreground"
                  }`}
                  onClick={() => selectDrive(drive)}
                >
                  <HardDrive className={`h-4 w-4 flex-shrink-0 ${isSelected ? "text-primary" : "text-muted-foreground"}`} />
                  <div className="flex-1 min-w-0">
                    <p className="truncate">{name}</p>
                    <p className="text-xs text-muted-foreground">
                      {formatBytes(Number(drive.maxCapacity))}
                    </p>
                  </div>
                  <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                    <button
                      data-testid={`drive-list-access-${drive.driveId}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        setAccessDrive(drive);
                      }}
                      className="text-muted-foreground hover:text-foreground"
                    >
                      <Users className="h-3.5 w-3.5" />
                    </button>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <button
                          data-testid={`drive-list-menu-${drive.driveId}`}
                          onClick={(e) => e.stopPropagation()}
                          className="text-muted-foreground hover:text-foreground"
                        >
                          <MoreHorizontal className="h-3.5 w-3.5" />
                        </button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
                        <DropdownMenuItem
                          data-testid={`drive-list-delete-${drive.driveId}`}
                          onClick={() => setDriveToDelete(drive)}
                          className="text-destructive focus:text-destructive"
                        >
                          <Trash2 className="mr-2 h-4 w-4" />
                          Delete drive
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </div>
              </ContextMenuTrigger>
              <ContextMenuContent>
                <ContextMenuItem onClick={() => setAccessDrive(drive)}>
                  <Users className="mr-2 h-4 w-4" />
                  Manage access
                </ContextMenuItem>
                <ContextMenuSeparator />
                <ContextMenuItem
                  onClick={() => setDriveToDelete(drive)}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="mr-2 h-4 w-4" />
                  Delete drive
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          );
        })}
      </div>

      <NewDriveDialog open={showNewDrive} onOpenChange={setShowNewDrive} />

      <ConfirmDialog
        open={!!driveToDelete}
        onOpenChange={() => setDriveToDelete(null)}
        title={`Delete "${driveToDelete?.name ?? `Drive ${driveToDelete?.driveId}`}"?`}
        description="This will permanently delete the drive and all its files. Storage agreements will be terminated with prorated refunds."
        onConfirm={handleDelete}
      />

      {accessDrive && (
        <ManageAccessDialog
          open={!!accessDrive}
          onOpenChange={() => setAccessDrive(null)}
          bucketId={accessDrive.bucketId}
          driveName={accessDrive.name ?? `Drive ${accessDrive.driveId}`}
        />
      )}
    </div>
  );
}
