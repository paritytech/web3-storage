import { useState, useCallback, useEffect } from "react";
import {
  Upload as UploadIcon,
  File,
  X,
  CheckCircle,
  AlertCircle,
  Loader2,
  HardDrive,
  Archive,
} from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
import { toast } from "@/components/ui/toaster";
import { formatBytes } from "@/lib/utils";
import type { DriveInfo, BucketInfo } from "@/lib/storage";

type UploadTarget = "drive" | "bucket";

interface UploadFile {
  id: string;
  file: File;
  progress: number;
  status: "pending" | "uploading" | "completed" | "error";
  cid?: string;
  error?: string;
}

export default function Upload() {
  const { connected } = useChain();
  const {
    signerAddress,
    drives,
    buckets,
    loading,
    refreshDrives,
    refreshBuckets,
    uploadToDrive,
    putObject,
  } = useStorage();

  const [uploadTarget, setUploadTarget] = useState<UploadTarget>("drive");
  const [selectedDrive, setSelectedDrive] = useState<DriveInfo | null>(null);
  const [selectedBucket, setSelectedBucket] = useState<BucketInfo | null>(null);
  const [targetPath, setTargetPath] = useState("/");
  const [files, setFiles] = useState<UploadFile[]>([]);
  const [uploading, setUploading] = useState(false);

  // Refresh drives/buckets on mount
  useEffect(() => {
    if (signerAddress && connected) {
      refreshDrives();
      refreshBuckets();
    }
  }, [signerAddress, connected, refreshDrives, refreshBuckets]);

  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    const droppedFiles = Array.from(e.dataTransfer.files);
    addFiles(droppedFiles);
  }, []);

  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    if (e.target.files) {
      addFiles(Array.from(e.target.files));
    }
  };

  const addFiles = (newFiles: File[]) => {
    const uploadFiles: UploadFile[] = newFiles.map((file) => ({
      id: `${file.name}-${Date.now()}-${Math.random()}`,
      file,
      progress: 0,
      status: "pending",
    }));
    setFiles((prev) => [...prev, ...uploadFiles]);
  };

  const removeFile = (id: string) => {
    setFiles((prev) => prev.filter((f) => f.id !== id));
  };

  const readFileAsUint8Array = (file: File): Promise<Uint8Array> => {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        if (reader.result instanceof ArrayBuffer) {
          resolve(new Uint8Array(reader.result));
        } else {
          reject(new Error("Failed to read file"));
        }
      };
      reader.onerror = () => reject(reader.error);
      reader.readAsArrayBuffer(file);
    });
  };

  const handleUpload = async () => {
    if (uploadTarget === "drive" && !selectedDrive) {
      toast({
        title: "Error",
        description: "Please select a drive",
        variant: "destructive",
      });
      return;
    }

    if (uploadTarget === "bucket" && !selectedBucket) {
      toast({
        title: "Error",
        description: "Please select a bucket",
        variant: "destructive",
      });
      return;
    }

    if (files.length === 0) {
      toast({
        title: "Error",
        description: "Please select files to upload",
        variant: "destructive",
      });
      return;
    }

    setUploading(true);

    for (const uploadFile of files) {
      if (uploadFile.status !== "pending") continue;

      setFiles((prev) =>
        prev.map((f) =>
          f.id === uploadFile.id ? { ...f, status: "uploading" as const, progress: 10 } : f
        )
      );

      try {
        // Read file data
        const data = await readFileAsUint8Array(uploadFile.file);

        setFiles((prev) =>
          prev.map((f) => (f.id === uploadFile.id ? { ...f, progress: 30 } : f))
        );

        let result;

        if (uploadTarget === "drive" && selectedDrive) {
          // Upload to drive
          const path = targetPath.endsWith("/")
            ? `${targetPath}${uploadFile.file.name}`
            : `${targetPath}/${uploadFile.file.name}`;

          result = await uploadToDrive(
            selectedDrive.driveId,
            selectedDrive.bucketId,
            path,
            data
          );
        } else if (uploadTarget === "bucket" && selectedBucket) {
          // Upload to S3 bucket
          const key = targetPath.startsWith("/")
            ? `${targetPath.slice(1)}${uploadFile.file.name}`
            : `${targetPath}${uploadFile.file.name}`;

          result = await putObject(
            selectedBucket.name,
            key,
            data,
            selectedBucket.layer0BucketId,
            { contentType: uploadFile.file.type || "application/octet-stream" }
          );
        }

        setFiles((prev) =>
          prev.map((f) =>
            f.id === uploadFile.id
              ? { ...f, status: "completed" as const, progress: 100, cid: result?.cid }
              : f
          )
        );
      } catch (err) {
        setFiles((prev) =>
          prev.map((f) =>
            f.id === uploadFile.id
              ? {
                  ...f,
                  status: "error" as const,
                  error: err instanceof Error ? err.message : "Upload failed",
                }
              : f
          )
        );
      }
    }

    setUploading(false);

    const successCount = files.filter(f => f.status === "completed" || files.find(uf => uf.id === f.id && uf.status === "pending")).length;
    if (successCount > 0) {
      toast({ title: "Success", description: "Files uploaded successfully" });
    }
  };

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Upload</h1>
          <p className="text-muted-foreground">Upload files to storage</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <UploadIcon className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p className="text-muted-foreground">
              Connect to the network to upload files
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (!signerAddress) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Upload</h1>
          <p className="text-muted-foreground">Upload files to storage</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <AlertCircle className="mx-auto h-12 w-12 mb-4 text-yellow-500" />
            <p className="text-muted-foreground mb-2">No signer set</p>
            <p className="text-sm text-muted-foreground">
              Go to the Accounts page to set a signing account first
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Upload</h1>
        <p className="text-muted-foreground">Upload files to storage</p>
      </div>

      {/* Upload Target */}
      <Card>
        <CardHeader>
          <CardTitle>Upload Destination</CardTitle>
          <CardDescription>
            Choose where to upload your files
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex gap-4">
            <Button
              variant={uploadTarget === "drive" ? "default" : "outline"}
              onClick={() => setUploadTarget("drive")}
            >
              <HardDrive className="mr-2 h-4 w-4" />
              File System Drive
            </Button>
            <Button
              variant={uploadTarget === "bucket" ? "default" : "outline"}
              onClick={() => setUploadTarget("bucket")}
            >
              <Archive className="mr-2 h-4 w-4" />
              S3 Bucket
            </Button>
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium">
                {uploadTarget === "drive" ? "Select Drive" : "Select Bucket"}
              </label>
              {uploadTarget === "drive" ? (
                <select
                  className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                  value={selectedDrive?.driveId.toString() || ""}
                  onChange={(e) => {
                    const drive = drives.find(
                      (d) => d.driveId.toString() === e.target.value
                    );
                    setSelectedDrive(drive || null);
                  }}
                >
                  <option value="">Select a drive...</option>
                  {drives.map((drive) => (
                    <option key={drive.driveId.toString()} value={drive.driveId.toString()}>
                      {drive.name || `Drive ${drive.driveId.toString()}`}
                    </option>
                  ))}
                </select>
              ) : (
                <select
                  className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                  value={selectedBucket?.s3BucketId.toString() || ""}
                  onChange={(e) => {
                    const bucket = buckets.find(
                      (b) => b.s3BucketId.toString() === e.target.value
                    );
                    setSelectedBucket(bucket || null);
                  }}
                >
                  <option value="">Select a bucket...</option>
                  {buckets.map((bucket) => (
                    <option key={bucket.s3BucketId.toString()} value={bucket.s3BucketId.toString()}>
                      {bucket.name}
                    </option>
                  ))}
                </select>
              )}
              {uploadTarget === "drive" && drives.length === 0 && (
                <p className="text-xs text-muted-foreground">
                  No drives found. Create one in the Drives page.
                </p>
              )}
              {uploadTarget === "bucket" && buckets.length === 0 && (
                <p className="text-xs text-muted-foreground">
                  No buckets found. Create one in the Buckets page.
                </p>
              )}
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">
                {uploadTarget === "drive" ? "Path" : "Key Prefix"}
              </label>
              <Input
                placeholder={uploadTarget === "drive" ? "/" : "uploads/"}
                value={targetPath}
                onChange={(e) => setTargetPath(e.target.value)}
              />
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Drop Zone */}
      <Card>
        <CardHeader>
          <CardTitle>Select Files</CardTitle>
          <CardDescription>
            Drag and drop files or click to select
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div
            className="rounded-lg border-2 border-dashed p-8 text-center transition-colors hover:border-primary"
            onDragOver={(e) => e.preventDefault()}
            onDrop={handleDrop}
          >
            <UploadIcon className="mx-auto h-12 w-12 mb-4 text-muted-foreground" />
            <p className="mb-2">Drag and drop files here</p>
            <p className="text-sm text-muted-foreground mb-4">or</p>
            <label>
              <input
                type="file"
                multiple
                className="hidden"
                onChange={handleFileSelect}
              />
              <Button variant="outline" asChild>
                <span>Select Files</span>
              </Button>
            </label>
          </div>
        </CardContent>
      </Card>

      {/* File List */}
      {files.length > 0 && (
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle>Files ({files.length})</CardTitle>
              <Button
                onClick={handleUpload}
                disabled={uploading || loading || files.every((f) => f.status !== "pending")}
              >
                {uploading ? (
                  <>
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    Uploading...
                  </>
                ) : (
                  <>
                    <UploadIcon className="mr-2 h-4 w-4" />
                    Upload All
                  </>
                )}
              </Button>
            </div>
          </CardHeader>
          <CardContent>
            <div className="space-y-3">
              {files.map((uploadFile) => (
                <div
                  key={uploadFile.id}
                  className="flex items-center gap-4 rounded-lg border p-3"
                >
                  <File className="h-8 w-8 text-muted-foreground" />
                  <div className="flex-1 min-w-0">
                    <p className="font-medium truncate">{uploadFile.file.name}</p>
                    <p className="text-sm text-muted-foreground">
                      {formatBytes(uploadFile.file.size)}
                    </p>
                    {uploadFile.status === "uploading" && (
                      <div className="mt-2 h-1.5 w-full rounded-full bg-secondary">
                        <div
                          className="h-full rounded-full bg-primary transition-all"
                          style={{ width: `${uploadFile.progress}%` }}
                        />
                      </div>
                    )}
                    {uploadFile.cid && (
                      <p className="mt-1 font-mono text-xs text-muted-foreground truncate">
                        CID: {uploadFile.cid}
                      </p>
                    )}
                    {uploadFile.error && (
                      <p className="mt-1 text-xs text-destructive">
                        {uploadFile.error}
                      </p>
                    )}
                  </div>
                  <div className="flex items-center gap-2">
                    {uploadFile.status === "completed" && (
                      <CheckCircle className="h-5 w-5 text-green-500" />
                    )}
                    {uploadFile.status === "error" && (
                      <AlertCircle className="h-5 w-5 text-destructive" />
                    )}
                    {uploadFile.status === "uploading" && (
                      <Loader2 className="h-5 w-5 animate-spin text-primary" />
                    )}
                    {uploadFile.status === "pending" && (
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => removeFile(uploadFile.id)}
                      >
                        <X className="h-4 w-4" />
                      </Button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
