import { useRef, useState, useCallback, type ReactNode } from "react";
import { Upload, Loader2 } from "lucide-react";
import { useDrive } from "@/hooks/useDrive";

interface UploadZoneProps {
  children: ReactNode;
}

export default function UploadZone({ children }: UploadZoneProps) {
  const { uploadFiles, uploading, selectedDrive } = useDrive();
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
        await uploadFiles(files);
      }
    },
    [selectedDrive, uploadFiles]
  );

  const handleFileSelect = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const files = Array.from(e.target.files || []);
      if (files.length > 0) {
        await uploadFiles(files);
      }
      // Reset input so same file can be selected again
      e.target.value = "";
    },
    [uploadFiles]
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

      {/* Hidden file input */}
      <input
        ref={fileInputRef}
        type="file"
        multiple
        className="hidden"
        onChange={handleFileSelect}
      />

      {/* Drag overlay */}
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
    </div>
  );
}

export function useFileUploadTrigger() {
  const ref = useRef<HTMLInputElement>(null);

  const trigger = useCallback(() => {
    ref.current?.click();
  }, []);

  return { ref, trigger };
}
