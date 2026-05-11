import { useRef, useState, useCallback, type ReactNode } from "react";
import { Upload, Loader2, X } from "lucide-react";
import { useUploading, useSelectedDrive, uploadFiles, abortUpload } from "@/state";

interface UploadZoneProps {
  children: ReactNode;
}

export default function UploadZone({ children }: UploadZoneProps) {
  const uploading = useUploading();
  const selectedDrive = useSelectedDrive();
  const [dragOver, setDragOver] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const dragCounter = useRef(0);

  const handleDragEnter = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounter.current++;
    if (e.dataTransfer.items.length > 0) {
      setDragOver(true);
    }
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounter.current--;
    if (dragCounter.current === 0) {
      setDragOver(false);
    }
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
  }, []);

  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setDragOver(false);
      dragCounter.current = 0;

      if (!selectedDrive) return;

      const files = Array.from(e.dataTransfer.files);
      if (files.length > 0) {
        await uploadFiles(files).catch(() => { /* error surfaced via state */ });
      }
    },
    [selectedDrive],
  );

  const handleFileSelect = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const files = Array.from(e.target.files || []);
      if (files.length > 0) {
        await uploadFiles(files).catch(() => { /* swallow */ });
      }
      e.target.value = "";
    },
    [],
  );

  return (
    <div
      className="relative flex-1"
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      {children}

      <input
        ref={fileInputRef}
        data-testid="upload-input"
        type="file"
        multiple
        className="hidden"
        onChange={handleFileSelect}
      />

      {dragOver && selectedDrive && (
        <div className="absolute inset-0 z-50 flex items-center justify-center rounded-lg border-2 border-dashed border-primary bg-primary/5 backdrop-blur-sm">
          <div className="flex flex-col items-center gap-2 text-primary">
            {uploading ? (
              <Loader2 className="h-10 w-10 animate-spin" />
            ) : (
              <Upload className="h-10 w-10" />
            )}
            <p className="text-sm font-medium">
              {uploading ? "Uploading..." : "Drop files here to upload"}
            </p>
          </div>
        </div>
      )}

      {uploading && !dragOver && (
        <button
          data-testid="upload-cancel"
          onClick={abortUpload}
          className="absolute bottom-4 right-4 z-50 flex items-center gap-2 rounded-full bg-background border px-3 py-1.5 text-xs font-medium shadow-md hover:bg-accent"
        >
          <X className="h-3 w-3" />
          Cancel upload
        </button>
      )}
    </div>
  );
}
