import { useState, useEffect, useCallback, useRef } from "react";
import {
  HardDrive,
  Plus,
  Folder,
  File,
  FileText,
  Image,
  RefreshCw,
  Trash2,
  Download,
  Upload,
  FolderPlus,
  ChevronRight,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useStorage } from "@/hooks/useStorage";
import { toast } from "@/components/ui/toaster";
import { formatBytes } from "@/lib/utils";
import type { DriveInfo, FsEntryInfo } from "@/lib/storage";

function fileIcon(name: string) {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp"].includes(ext))
    return <Image className="h-4 w-4 text-purple-500" />;
  if (["txt", "md", "json", "toml", "yaml", "yml", "csv", "log", "rs", "ts", "tsx", "js"].includes(ext))
    return <FileText className="h-4 w-4 text-blue-500" />;
  return <File className="h-4 w-4 text-muted-foreground" />;
}

interface FileSystemTabProps {
  onBucketSelect?: (bucketId: bigint | null) => void;
}

export default function FileSystemTab({ onBucketSelect }: FileSystemTabProps) {
  const {
    client,
    drives,
    loading,
    refreshDrives,
    createDrive,
    deleteDrive,
    uploadToDrive,
    listDriveFiles,
    createDirectory,
    deleteFile,
    signerAddress,
    waitForProvider,
  } = useStorage();

  const [selectedDrive, setSelectedDrive] = useState<DriveInfo | null>(null);
  const [currentPath, setCurrentPath] = useState("/");
  const [entries, setEntries] = useState<FsEntryInfo[]>([]);
  const [loadingEntries, setLoadingEntries] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Create drive dialog state
  const [showCreateDrive, setShowCreateDrive] = useState(false);
  const [newDriveName, setNewDriveName] = useState("");
  // Defaults: 10 MB capacity, 500 blocks duration
  // maxPayment must cover: price_per_byte(1e6) × capacity × duration × 1.2 buffer
  const [driveCapacity, setDriveCapacity] = useState("10000000");
  const [driveDuration, setDriveDuration] = useState("500");
  const [driveMaxPayment, setDriveMaxPayment] = useState("6000000000000000");
  const [creating, setCreating] = useState(false);
  const [providerStatus, setProviderStatus] = useState<{ message: string; progress: number } | null>(null);

  // New folder state
  const [showNewFolder, setShowNewFolder] = useState(false);
  const [newFolderName, setNewFolderName] = useState("");

  // Refresh drives on mount
  useEffect(() => {
    if (signerAddress) refreshDrives();
  }, [signerAddress, refreshDrives]);

  // Auto-select first drive
  useEffect(() => {
    if (drives.length > 0 && !selectedDrive) {
      setSelectedDrive(drives[0]);
      onBucketSelect?.(drives[0].bucketId);
    }
  }, [drives, selectedDrive, onBucketSelect]);

  // Fetch entries when drive/path changes
  const refreshEntries = useCallback(async () => {
    if (!selectedDrive) return;
    setLoadingEntries(true);
    try {
      const list = await listDriveFiles(selectedDrive.bucketId, currentPath === "/" ? undefined : currentPath);
      setEntries(list);
    } catch {
      setEntries([]);
    } finally {
      setLoadingEntries(false);
    }
  }, [selectedDrive, currentPath, listDriveFiles]);

  useEffect(() => {
    refreshEntries();
  }, [refreshEntries]);

  const handleCreateDrive = async () => {
    setCreating(true);
    try {
      const driveId = await createDrive({
        name: newDriveName || undefined,
        capacity: BigInt(driveCapacity),
        duration: parseInt(driveDuration, 10),
        maxPayment: BigInt(driveMaxPayment),
      });
      setShowCreateDrive(false);
      setNewDriveName("");
      toast({ title: "Drive created", description: "Waiting for provider to accept agreement..." });
      await refreshDrives();

      // Look up the bucket ID for the new drive and wait for provider
      if (client) {
        const drive = await client.getDrive(driveId);
        if (drive) {
          setProviderStatus({ message: "Waiting for provider...", progress: 0 });
          try {
            await waitForProvider(drive.bucketId, (_status, attempt, total) => {
              setProviderStatus({
                message: `Waiting for provider to accept agreement...`,
                progress: Math.round((attempt / total) * 100),
              });
            });
            setProviderStatus(null);
            toast({ title: "Ready", description: "Provider connected — drive is ready to use" });
          } catch {
            setProviderStatus(null);
            toast({ title: "Warning", description: "Provider not yet available. Operations may fail until the provider accepts.", variant: "destructive" });
          }
        }
      }
    } catch (err) {
      toast({ title: "Error", description: err instanceof Error ? err.message : "Failed to create drive", variant: "destructive" });
    } finally {
      setCreating(false);
    }
  };

  const handleDeleteDrive = async () => {
    if (!selectedDrive) return;
    try {
      await deleteDrive(selectedDrive.driveId);
      setSelectedDrive(null);
      setEntries([]);
      onBucketSelect?.(null);
      toast({ title: "Success", description: "Drive deleted" });
    } catch (err) {
      toast({ title: "Error", description: err instanceof Error ? err.message : "Failed", variant: "destructive" });
    }
  };

  const navigateTo = (path: string) => {
    setCurrentPath(path);
  };

  const breadcrumbSegments = () => {
    if (currentPath === "/") return [{ name: "Root", path: "/" }];
    const parts = currentPath.split("/").filter(Boolean);
    const segments = [{ name: "Root", path: "/" }];
    let acc = "";
    for (const part of parts) {
      acc += "/" + part;
      segments.push({ name: part, path: acc });
    }
    return segments;
  };

  const readFileAsUint8Array = (file: globalThis.File): Promise<Uint8Array> => {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        if (reader.result instanceof ArrayBuffer) resolve(new Uint8Array(reader.result));
        else reject(new Error("Failed to read file"));
      };
      reader.onerror = () => reject(reader.error);
      reader.readAsArrayBuffer(file);
    });
  };

  const uploadFiles = async (files: globalThis.File[]) => {
    if (!selectedDrive) return;
    for (const file of files) {
      try {
        const data = await readFileAsUint8Array(file);
        const path = currentPath === "/" ? `/${file.name}` : `${currentPath}/${file.name}`;
        await uploadToDrive(selectedDrive.driveId, selectedDrive.bucketId, path, data);
        toast({ title: "Uploaded", description: file.name });
      } catch (err) {
        toast({ title: "Upload failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
      }
    }
    refreshEntries();
  };

  const handleFileDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    const files = Array.from(e.dataTransfer.files);
    if (files.length > 0) uploadFiles(files);
  };

  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    if (e.target.files) {
      uploadFiles(Array.from(e.target.files));
      e.target.value = "";
    }
  };

  const handleDownload = async (entry: FsEntryInfo) => {
    if (!selectedDrive || !client) return;
    try {
      const blob = await client.downloadFile(selectedDrive.bucketId, entry.path);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = entry.name;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      toast({ title: "Downloaded", description: entry.name });
    } catch (err) {
      toast({ title: "Download failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    }
  };

  const handleDeleteFile = async (entry: FsEntryInfo) => {
    if (!selectedDrive) return;
    try {
      await deleteFile(selectedDrive.bucketId, entry.path);
      toast({ title: "Deleted", description: entry.name });
      refreshEntries();
    } catch (err) {
      toast({ title: "Delete failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    }
  };

  const handleNewFolder = async () => {
    if (!selectedDrive || !newFolderName.trim()) return;
    try {
      const path = currentPath === "/" ? `/${newFolderName}` : `${currentPath}/${newFolderName}`;
      await createDirectory(selectedDrive.bucketId, path);
      setShowNewFolder(false);
      setNewFolderName("");
      toast({ title: "Created", description: `Folder "${newFolderName}"` });
      refreshEntries();
    } catch (err) {
      toast({ title: "Error", description: err instanceof Error ? err.message : "Failed", variant: "destructive" });
    }
  };

  // No drives empty state
  if (drives.length === 0 && !showCreateDrive) {
    return (
      <div className="py-12 text-center">
        <HardDrive className="mx-auto h-12 w-12 mb-4 opacity-50" />
        <p className="text-muted-foreground mb-4">No drives yet. Create your first drive to start organizing files.</p>
        <Button onClick={() => setShowCreateDrive(true)}>
          <Plus className="mr-2 h-4 w-4" />
          Create Drive
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {/* Drive bar */}
      <div className="flex items-center gap-3">
        <select
          className="flex-1 max-w-xs rounded-md border border-input bg-background px-3 py-2 text-sm"
          value={selectedDrive?.driveId.toString() || ""}
          onChange={(e) => {
            const drive = drives.find((d) => d.driveId.toString() === e.target.value);
            setSelectedDrive(drive || null);
            setCurrentPath("/");
            onBucketSelect?.(drive?.bucketId ?? null);
          }}
        >
          <option value="">Select a drive...</option>
          {drives.map((drive) => (
            <option key={drive.driveId.toString()} value={drive.driveId.toString()}>
              {drive.name || `Drive ${drive.driveId.toString()}`}
            </option>
          ))}
        </select>
        <Button variant="outline" size="sm" onClick={() => setShowCreateDrive(true)}>
          <Plus className="mr-2 h-4 w-4" />
          New Drive
        </Button>
        {selectedDrive && (
          <Button variant="ghost" size="sm" onClick={handleDeleteDrive} className="text-muted-foreground hover:text-destructive">
            <Trash2 className="h-4 w-4" />
          </Button>
        )}
      </div>

      {/* Create Drive Dialog */}
      {showCreateDrive && (
        <Card>
          <CardHeader>
            <CardTitle>Create New Drive</CardTitle>
            <CardDescription>Set up a new file system drive</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <Input placeholder="Drive name (optional)" value={newDriveName} onChange={(e) => setNewDriveName(e.target.value)} />
            <div className="grid gap-3 md:grid-cols-3">
              <div className="space-y-1">
                <label className="text-xs font-medium">Capacity (bytes)</label>
                <Input type="number" value={driveCapacity} onChange={(e) => setDriveCapacity(e.target.value)} />
                <p className="text-xs text-muted-foreground">{formatBytes(parseInt(driveCapacity, 10) || 0)}</p>
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium">Duration (blocks)</label>
                <Input type="number" value={driveDuration} onChange={(e) => setDriveDuration(e.target.value)} />
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium">Max Payment</label>
                <Input type="number" value={driveMaxPayment} onChange={(e) => setDriveMaxPayment(e.target.value)} />
              </div>
            </div>
            <div className="flex gap-2">
              <Button onClick={handleCreateDrive} disabled={creating || loading}>
                {creating ? "Creating..." : "Create"}
              </Button>
              <Button variant="ghost" onClick={() => setShowCreateDrive(false)}>Cancel</Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Provider waiting indicator */}
      {providerStatus && (
        <div className="rounded-lg border bg-card p-4">
          <div className="flex items-center gap-3">
            <RefreshCw className="h-4 w-4 animate-spin text-primary" />
            <div className="flex-1">
              <p className="text-sm font-medium">{providerStatus.message}</p>
              <div className="mt-2 h-2 rounded-full bg-secondary">
                <div
                  className="h-full rounded-full bg-primary transition-all duration-500"
                  style={{ width: `${providerStatus.progress}%` }}
                />
              </div>
            </div>
          </div>
        </div>
      )}

      {selectedDrive && (
        <>
          {/* Breadcrumbs */}
          <div className="flex items-center gap-1 text-sm">
            {breadcrumbSegments().map((seg, i, arr) => (
              <span key={seg.path} className="flex items-center gap-1">
                {i > 0 && <ChevronRight className="h-3 w-3 text-muted-foreground" />}
                <button
                  className={`hover:underline ${i === arr.length - 1 ? "font-medium" : "text-muted-foreground"}`}
                  onClick={() => navigateTo(seg.path)}
                >
                  {seg.name}
                </button>
              </span>
            ))}
          </div>

          {/* Toolbar */}
          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => setShowNewFolder(true)}>
              <FolderPlus className="mr-2 h-4 w-4" />
              New Folder
            </Button>
            <Button variant="outline" size="sm" onClick={() => fileInputRef.current?.click()}>
              <Upload className="mr-2 h-4 w-4" />
              Upload Files
            </Button>
            <input ref={fileInputRef} type="file" multiple className="hidden" onChange={handleFileSelect} />
            <Button variant="ghost" size="sm" onClick={refreshEntries} disabled={loadingEntries}>
              <RefreshCw className={`h-4 w-4 ${loadingEntries ? "animate-spin" : ""}`} />
            </Button>
          </div>

          {/* New Folder inline */}
          {showNewFolder && (
            <div className="flex items-center gap-2">
              <Input
                placeholder="Folder name"
                value={newFolderName}
                onChange={(e) => setNewFolderName(e.target.value)}
                className="max-w-xs"
                autoFocus
                onKeyDown={(e) => { if (e.key === "Enter") handleNewFolder(); if (e.key === "Escape") setShowNewFolder(false); }}
              />
              <Button size="sm" onClick={handleNewFolder}>Create</Button>
              <Button variant="ghost" size="sm" onClick={() => { setShowNewFolder(false); setNewFolderName(""); }}>Cancel</Button>
            </div>
          )}

          {/* File/folder list */}
          <div
            className={`rounded-lg border ${dragOver ? "border-primary bg-primary/5" : ""}`}
            onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
            onDragLeave={() => setDragOver(false)}
            onDrop={handleFileDrop}
          >
            {loadingEntries ? (
              <div className="p-8 text-center text-muted-foreground">
                <RefreshCw className="mx-auto h-8 w-8 mb-2 animate-spin opacity-50" />
                <p>Loading...</p>
              </div>
            ) : entries.length === 0 ? (
              <div className="p-8 text-center text-muted-foreground">
                <Upload className="mx-auto h-8 w-8 mb-2 opacity-50" />
                <p>This folder is empty</p>
                <p className="text-sm">Drop files here or use the toolbar to upload</p>
              </div>
            ) : (
              <table className="w-full">
                <thead>
                  <tr className="border-b bg-muted/50">
                    <th className="px-4 py-2 text-left text-sm font-medium">Name</th>
                    <th className="px-4 py-2 text-left text-sm font-medium w-24">Size</th>
                    <th className="px-4 py-2 text-left text-sm font-medium w-44">Modified</th>
                    <th className="px-4 py-2 text-right text-sm font-medium w-24">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {/* Directories first, then files */}
                  {[...entries].sort((a, b) => {
                    if (a.entryType === "directory" && b.entryType !== "directory") return -1;
                    if (a.entryType !== "directory" && b.entryType === "directory") return 1;
                    return a.name.localeCompare(b.name);
                  }).map((entry) => (
                    <tr key={entry.path} className="border-b hover:bg-muted/30">
                      <td className="px-4 py-2">
                        {entry.entryType === "directory" ? (
                          <button
                            className="flex items-center gap-2 hover:underline"
                            onClick={() => navigateTo(entry.path)}
                          >
                            <Folder className="h-4 w-4 text-yellow-500" />
                            <span className="text-sm font-medium">{entry.name}</span>
                          </button>
                        ) : (
                          <span className="flex items-center gap-2 text-sm">
                            {fileIcon(entry.name)}
                            {entry.name}
                          </span>
                        )}
                      </td>
                      <td className="px-4 py-2 text-sm text-muted-foreground">
                        {entry.entryType === "file" ? formatBytes(entry.size) : "--"}
                      </td>
                      <td className="px-4 py-2 text-sm text-muted-foreground">
                        {entry.mtime ? new Date(entry.mtime).toLocaleString() : "--"}
                      </td>
                      <td className="px-4 py-2 text-right">
                        <div className="flex items-center justify-end gap-1">
                          {entry.entryType === "file" && (
                            <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => handleDownload(entry)}>
                              <Download className="h-3.5 w-3.5" />
                            </Button>
                          )}
                          <Button variant="ghost" size="icon" className="h-7 w-7 text-muted-foreground hover:text-destructive" onClick={() => handleDeleteFile(entry)}>
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </>
      )}
    </div>
  );
}
