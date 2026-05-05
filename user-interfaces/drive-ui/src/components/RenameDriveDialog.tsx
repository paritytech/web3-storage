import { useState } from "react";
import { Loader2 } from "lucide-react";
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

interface RenameDriveDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  currentName: string;
  driveId: bigint;
  onConfirm: (newName: string) => Promise<void> | void;
}

export default function RenameDriveDialog({
  open,
  onOpenChange,
  currentName,
  driveId,
  onConfirm,
}: RenameDriveDialogProps) {
  const [name, setName] = useState(currentName);
  const [submitting, setSubmitting] = useState(false);

  const handleSubmit = async () => {
    if (!name.trim()) return;
    setSubmitting(true);
    try {
      await onConfirm(name.trim());
      onOpenChange(false);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md" data-testid="rename-dialog">
        <DialogHeader>
          <DialogTitle>Rename drive</DialogTitle>
          <DialogDescription>
            Update the name of drive #{driveId.toString()}.
          </DialogDescription>
        </DialogHeader>
        <Input
          data-testid="rename-input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="My Documents"
          onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
          autoFocus
        />
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={submitting}>
            Cancel
          </Button>
          <Button
            data-testid="rename-submit"
            onClick={handleSubmit}
            disabled={submitting || !name.trim() || name.trim() === currentName}
          >
            {submitting ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
            Rename
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
