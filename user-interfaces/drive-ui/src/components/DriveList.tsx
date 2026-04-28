import { useState } from "react";
import { HardDrive, Plus, Trash2, Users } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useDrive } from "@/hooks/useDrive";
import { formatBytes } from "@/lib/utils";
import ConfirmDialog from "./ConfirmDialog";
import NewDriveDialog from "./NewDriveDialog";
import ManageAccessDialog from "./ManageAccessDialog";

export default function DriveList() {
  const { drives, selectedDrive, selectDrive, deleteDrive } = useDrive();
  const [showNewDrive, setShowNewDrive] = useState(false);
  const [driveToDelete, setDriveToDelete] = useState<{
    id: bigint;
    name: string;
  } | null>(null);
  const [accessDrive, setAccessDrive] = useState<{
    bucketId: bigint;
    name: string;
  } | null>(null);

  return (
    <div className="flex flex-col gap-2">
      <Button
        onClick={() => setShowNewDrive(true)}
        size="sm"
        className="w-full"
      >
        <Plus className="mr-2 h-4 w-4" />
        New Drive
      </Button>

      <div className="flex flex-col gap-1 mt-2">
        {drives.map((drive) => {
          const isSelected = selectedDrive?.driveId === drive.driveId;
          const name = drive.name || `Drive ${drive.driveId}`;

          return (
            <div
              key={drive.driveId.toString()}
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
                  onClick={(e) => {
                    e.stopPropagation();
                    setAccessDrive({ bucketId: drive.bucketId, name });
                  }}
                  className="text-muted-foreground hover:text-foreground"
                >
                  <Users className="h-3.5 w-3.5" />
                </button>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    setDriveToDelete({ id: drive.driveId, name });
                  }}
                  className="text-muted-foreground hover:text-destructive"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          );
        })}
      </div>

      <NewDriveDialog open={showNewDrive} onOpenChange={setShowNewDrive} />

      <ConfirmDialog
        open={!!driveToDelete}
        onOpenChange={() => setDriveToDelete(null)}
        title={`Delete "${driveToDelete?.name}"?`}
        description="This will permanently delete the drive and all its files. Storage agreements will be terminated with prorated refunds."
        onConfirm={() => {
          if (driveToDelete) deleteDrive(driveToDelete.id);
        }}
      />

      {accessDrive && (
        <ManageAccessDialog
          open={!!accessDrive}
          onOpenChange={() => setAccessDrive(null)}
          bucketId={accessDrive.bucketId}
          driveName={accessDrive.name}
        />
      )}
    </div>
  );
}
