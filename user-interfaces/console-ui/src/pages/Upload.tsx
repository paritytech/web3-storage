import { useState, useCallback } from "react";
import {
  Upload as UploadIcon,
  File,
  X,
  CheckCircle,
  AlertCircle,
  Loader2,
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
import { toast } from "@/components/ui/toaster";
import { formatBytes } from "@/lib/utils";

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
  const [uploadTarget, setUploadTarget] = useState<UploadTarget>("drive");
  const [targetName, setTargetName] = useState("");
  const [targetPath, setTargetPath] = useState("/");
  const [files, setFiles] = useState<UploadFile[]>([]);
  const [uploading, setUploading] = useState(false);

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

  const handleUpload = async () => {
    if (!targetName.trim()) {
      toast({
        title: "Error",
        description: `Please select a ${uploadTarget}`,
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
          f.id === uploadFile.id ? { ...f, status: "uploading" as const } : f
        )
      );

      try {
        // TODO: Call SDK to upload file
        // Simulate upload progress
        for (let i = 0; i <= 100; i += 10) {
          await new Promise((r) => setTimeout(r, 100));
          setFiles((prev) =>
            prev.map((f) => (f.id === uploadFile.id ? { ...f, progress: i } : f))
          );
        }

        // Generate mock CID
        const mockCid = `0x${Array.from({ length: 64 }, () =>
          Math.floor(Math.random() * 16).toString(16)
        ).join("")}`;

        setFiles((prev) =>
          prev.map((f) =>
            f.id === uploadFile.id
              ? { ...f, status: "completed" as const, cid: mockCid }
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
    toast({ title: "Success", description: "Files uploaded successfully" });
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
              File System Drive
            </Button>
            <Button
              variant={uploadTarget === "bucket" ? "default" : "outline"}
              onClick={() => setUploadTarget("bucket")}
            >
              S3 Bucket
            </Button>
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium">
                {uploadTarget === "drive" ? "Drive Name" : "Bucket Name"}
              </label>
              <Input
                placeholder={uploadTarget === "drive" ? "my-drive" : "my-bucket"}
                value={targetName}
                onChange={(e) => setTargetName(e.target.value)}
              />
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
                disabled={uploading || files.every((f) => f.status !== "pending")}
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
