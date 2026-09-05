// SPDX-License-Identifier: GPL-3.0-only

import { useState, useRef, useCallback, useEffect } from "react";
import { Upload, FileUp, Loader2, Lock } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useCurrentPrefix, useEncryptionKey, uploadObject } from "@/state";
import { formatBytes } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";

interface UploadObjectDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialFile?: File | null;
}

export default function UploadObjectDialog({
  open,
  onOpenChange,
  initialFile,
}: UploadObjectDialogProps) {
  const currentPrefix = useCurrentPrefix();
  const encryptionKey = useEncryptionKey();

  const [key, setKey] = useState(currentPrefix);
  const [file, setFile] = useState<File | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const keyEdited = useRef(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [dragOver, setDragOver] = useState(false);

  // Reset state when dialog opens
  useEffect(() => {
    if (open) {
      keyEdited.current = false;
      setSubmitting(false);
      setDragOver(false);
      if (initialFile) {
        setFile(initialFile);
        setKey(`${currentPrefix}${initialFile.name}`);
      } else {
        setFile(null);
        setKey(currentPrefix);
      }
    }
  }, [open, initialFile, currentPrefix]);

  const handleKeyChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      keyEdited.current = true;
      setKey(e.target.value);
    },
    [],
  );

  const handleFileSelect = useCallback(
    (selected: File) => {
      setFile(selected);
      if (!keyEdited.current) {
        setKey(`${currentPrefix}${selected.name}`);
      }
    },
    [currentPrefix],
  );

  const handleInputChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const f = e.target.files?.[0];
      if (f) handleFileSelect(f);
      e.target.value = "";
    },
    [handleFileSelect],
  );

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setDragOver(false);
      const f = e.dataTransfer.files[0];
      if (f) handleFileSelect(f);
    },
    [handleFileSelect],
  );

  const canSubmit = key.trim().length > 0 && !key.endsWith("/") && file !== null && !submitting;

  const handleSubmit = useCallback(async () => {
    if (!file || !canSubmit) return;
    setSubmitting(true);
    try {
      await uploadObject(key, file);
      toast({
        title: "Upload complete",
        description: key,
      });
      onOpenChange(false);
    } catch (err) {
      toast({
        title: "Upload failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    } finally {
      setSubmitting(false);
    }
  }, [file, key, canSubmit, onOpenChange]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg" data-testid="upload-object-dialog">
        <DialogHeader>
          <DialogTitle>Upload Object</DialogTitle>
          <DialogDescription>
            Specify the destination key and choose a file to upload.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* Object Key */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Object Key</label>
            <Input
              data-testid="upload-object-key"
              value={key}
              onChange={handleKeyChange}
              placeholder={`${currentPrefix || ""}my-file.txt`}
              className="font-mono text-sm"
            />
          </div>

          {/* File picker / drop zone */}
          <div className="space-y-2">
            <label className="text-sm font-medium">File</label>
            <div
              data-testid="upload-drop-zone"
              className={`flex flex-col items-center justify-center gap-2 rounded-lg border-2 border-dashed p-6 transition-colors cursor-pointer ${
                dragOver
                  ? "border-primary bg-primary/5"
                  : "border-muted-foreground/25 hover:border-muted-foreground/50"
              }`}
              onClick={() => fileInputRef.current?.click()}
              onDragOver={(e) => {
                e.preventDefault();
                setDragOver(true);
              }}
              onDragLeave={() => setDragOver(false)}
              onDrop={handleDrop}
            >
              {file ? (
                <div className="flex items-center gap-3 text-sm">
                  <FileUp className="h-5 w-5 text-muted-foreground flex-shrink-0" />
                  <div className="min-w-0">
                    <p className="font-medium truncate">{file.name}</p>
                    <p className="text-xs text-muted-foreground">{formatBytes(file.size)}</p>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="ml-auto text-xs"
                    onClick={(e) => {
                      e.stopPropagation();
                      fileInputRef.current?.click();
                    }}
                  >
                    Change
                  </Button>
                </div>
              ) : (
                <>
                  <Upload className="h-8 w-8 text-muted-foreground" />
                  <p className="text-sm text-muted-foreground">
                    Click to choose or drag a file here
                  </p>
                </>
              )}
            </div>
            <input
              ref={fileInputRef}
              type="file"
              className="hidden"
              onChange={handleInputChange}
            />
          </div>

          {/* Encryption badge */}
          {encryptionKey && (
            <div className="flex items-center gap-2 rounded-md bg-amber-50 border border-amber-200 px-3 py-2 text-xs text-amber-800">
              <Lock className="h-3.5 w-3.5 flex-shrink-0" />
              Will be encrypted (AES-256-GCM)
            </div>
          )}

          {/* Footer */}
          <div className="flex justify-end gap-2 pt-2">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button
              data-testid="upload-object-submit"
              onClick={handleSubmit}
              disabled={!canSubmit}
            >
              {submitting ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Upload className="mr-2 h-4 w-4" />
              )}
              Upload
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
