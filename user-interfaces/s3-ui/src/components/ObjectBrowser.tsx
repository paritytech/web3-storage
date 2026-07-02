// SPDX-License-Identifier: GPL-3.0-only

import { useState, useCallback } from "react";
import {
  Upload,
  RefreshCw,
  ChevronRight,
  Folder,
  FileText,
  File as FileIcon,
  Download,
  Trash2,
  Copy,
  MoreHorizontal,
  LayoutGrid,
  LayoutList,
  Loader2,
  Shield,
  Lock,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from "@/components/ui/context-menu";
import {
  useSelectedBucket,
  useCurrentPrefix,
  useObjects,
  useS3Loading,
  useUploading,
  useViewMode,
  useEncryptionKey,
  setViewMode,
  navigateToPrefix,
  refreshObjects,
  downloadObject,
  deleteObject,
} from "@/state";
import { formatBytes, formatTimestamp } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";
import type { S3ObjectInfo } from "@/lib/s3-client";
import UploadZone from "./UploadZone";
import UploadObjectDialog from "./UploadObjectDialog";
import ConfirmDialog from "./ConfirmDialog";
import EmptyState from "./EmptyState";
import CheckpointPanel from "./CheckpointPanel";
import EncryptionPanel from "./EncryptionPanel";
import ChallengeHistoryPanel from "./ChallengeHistoryPanel";

/**
 * Derive virtual folders and files from flat S3 object keys.
 * Objects whose key starts with the current prefix are shown:
 * - Keys with another "/" after stripping the prefix become folder entries
 * - Keys without a "/" after stripping the prefix are file entries
 */
function deriveEntries(objects: S3ObjectInfo[], prefix: string) {
  const folders = new Set<string>();
  const files: S3ObjectInfo[] = [];

  for (const obj of objects) {
    const relative = prefix ? obj.key.slice(prefix.length) : obj.key;
    if (!relative) continue;

    const slashIdx = relative.indexOf("/");
    if (slashIdx >= 0) {
      // Virtual folder
      folders.add(relative.slice(0, slashIdx + 1));
    } else {
      files.push(obj);
    }
  }

  return {
    folders: Array.from(folders).sort(),
    files: files.sort((a, b) => a.key.localeCompare(b.key)),
  };
}

export default function ObjectBrowser() {
  const selectedBucket = useSelectedBucket();
  const currentPrefix = useCurrentPrefix();
  const objects = useObjects();
  const loading = useS3Loading();
  const uploading = useUploading();
  const viewMode = useViewMode();
  const encryptionKey = useEncryptionKey();

  const [objectToDelete, setObjectToDelete] = useState<string | null>(null);
  const [showCheckpoint, setShowCheckpoint] = useState(false);
  const [showEncryption, setShowEncryption] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [showUpload, setShowUpload] = useState(false);
  const [dropFile, setDropFile] = useState<File | null>(null);

  const handleDownload = useCallback(async (key: string) => {
    try {
      const data = await downloadObject(key);
      const blob = new Blob([data as BlobPart]);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      // Use the filename part of the key
      a.download = key.split("/").pop() || key;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      toast({ title: "Downloaded", description: key });
    } catch (err) {
      toast({
        title: "Download failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  }, []);

  const handleDelete = useCallback(async (key: string) => {
    try {
      await deleteObject(key);
      toast({ title: "Deleted", description: key });
    } catch (err) {
      toast({
        title: "Delete failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  }, []);

  const handleCopyKey = useCallback((key: string) => {
    const uri = selectedBucket
      ? `s3://${selectedBucket.name || selectedBucket.s3BucketId}/${key}`
      : key;
    navigator.clipboard.writeText(uri).then(
      () => toast({ title: "Copied", description: uri }),
      () => toast({ title: "Copy failed", variant: "destructive" }),
    );
  }, [selectedBucket]);

  if (!selectedBucket) return null;

  const { folders, files } = deriveEntries(objects, currentPrefix);

  const breadcrumbs = () => {
    const bucketName = selectedBucket.name || `Bucket ${selectedBucket.s3BucketId}`;
    const segments: { label: string; prefix: string }[] = [
      { label: bucketName, prefix: "" },
    ];
    if (currentPrefix) {
      const parts = currentPrefix.split("/").filter(Boolean);
      let acc = "";
      for (const part of parts) {
        acc += part + "/";
        segments.push({ label: part, prefix: acc });
      }
    }
    return segments;
  };

  return (
    <UploadZone onDropFiles={(files) => { setDropFile(files[0] ?? null); setShowUpload(true); }}>
      <div className="flex flex-col h-full" data-testid="object-browser">
        {/* Toolbar */}
        <div className="flex items-center justify-between gap-2 pb-4 border-b">
          <nav data-testid="breadcrumbs" className="flex items-center gap-1 text-sm min-w-0">
            {breadcrumbs().map((seg, i, arr) => (
              <span key={seg.prefix + i} className="flex items-center gap-1">
                {i > 0 && (
                  <ChevronRight className="h-3.5 w-3.5 text-muted-foreground flex-shrink-0" />
                )}
                <button
                  data-testid={`breadcrumb-${i}`}
                  className={`hover:underline truncate max-w-[150px] ${
                    i === arr.length - 1
                      ? "font-medium text-foreground"
                      : "text-muted-foreground"
                  }`}
                  onClick={() => navigateToPrefix(seg.prefix)}
                >
                  {seg.label}
                </button>
              </span>
            ))}
          </nav>

          <div className="flex items-center gap-1.5">
            <Button
              data-testid="upload-button"
              variant="outline"
              size="sm"
              onClick={() => { setDropFile(null); setShowUpload(true); }}
              disabled={uploading.active}
            >
              {uploading.active ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Upload className="mr-2 h-4 w-4" />
              )}
              Upload
            </Button>
            <div className="w-px h-6 bg-border mx-1" />
            <Button
              data-testid="encryption-toggle"
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              onClick={() => setShowEncryption(!showEncryption)}
              title={encryptionKey ? "Encryption enabled" : "Encryption"}
            >
              <Lock className={`h-4 w-4 ${encryptionKey ? "text-amber-500" : showEncryption ? "text-primary" : ""}`} />
            </Button>
            <Button
              data-testid="checkpoint-toggle"
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              onClick={() => setShowCheckpoint(!showCheckpoint)}
              title="Checkpoint"
            >
              <Shield className={`h-4 w-4 ${showCheckpoint ? "text-primary" : ""}`} />
            </Button>
            <Button
              data-testid="view-mode-toggle"
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              onClick={() => setViewMode(viewMode === "list" ? "grid" : "list")}
            >
              {viewMode === "list" ? (
                <LayoutGrid className="h-4 w-4" />
              ) : (
                <LayoutList className="h-4 w-4" />
              )}
            </Button>
            <Button
              data-testid="refresh-button"
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              onClick={() => refreshObjects()}
              disabled={loading}
            >
              <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
            </Button>
          </div>
        </div>

        {showEncryption && (
          <div className="pt-4">
            <EncryptionPanel />
          </div>
        )}

        {showCheckpoint && (
          <div className="pt-4">
            <CheckpointPanel onShowHistory={() => setShowHistory(true)} />
          </div>
        )}

        {/* Object list or challenge history */}
        {showHistory ? (
          <div className="flex-1 pt-4">
            <ChallengeHistoryPanel
              bucketId={selectedBucket.layer0BucketId ?? null}
              onClose={() => setShowHistory(false)}
            />
          </div>
        ) : <div className="flex-1 pt-4">
          {loading && folders.length === 0 && files.length === 0 ? (
            <div className="flex items-center justify-center py-16">
              <RefreshCw className="h-6 w-6 animate-spin text-muted-foreground" />
            </div>
          ) : folders.length === 0 && files.length === 0 ? (
            <EmptyState
              icon={<FileIcon className="h-12 w-12" />}
              title="This bucket is empty"
              description="Upload objects to get started."
              action={
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => { setDropFile(null); setShowUpload(true); }}
                >
                  <Upload className="mr-2 h-4 w-4" />
                  Upload Objects
                </Button>
              }
            />
          ) : viewMode === "list" ? (
            <ListView
              folders={folders}
              files={files}
              prefix={currentPrefix}
              onNavigate={navigateToPrefix}
              onDownload={handleDownload}
              onDelete={setObjectToDelete}
              onCopyKey={handleCopyKey}
            />
          ) : (
            <GridView
              folders={folders}
              files={files}
              prefix={currentPrefix}
              onNavigate={navigateToPrefix}
              onDownload={handleDownload}
              onDelete={setObjectToDelete}
              onCopyKey={handleCopyKey}
            />
          )}
        </div>}
      </div>

      <UploadObjectDialog
        open={showUpload}
        onOpenChange={setShowUpload}
        initialFile={dropFile}
      />

      <ConfirmDialog
        open={!!objectToDelete}
        onOpenChange={() => setObjectToDelete(null)}
        title={`Delete "${objectToDelete}"?`}
        description="This object will be removed from the bucket."
        onConfirm={() => {
          if (objectToDelete) handleDelete(objectToDelete);
        }}
      />
    </UploadZone>
  );
}

// ── List View ─────────────────────────────────────────────────────────────────

function ListView({
  folders,
  files,
  prefix,
  onNavigate,
  onDownload,
  onDelete,
  onCopyKey,
}: {
  folders: string[];
  files: S3ObjectInfo[];
  prefix: string;
  onNavigate: (prefix: string) => void;
  onDownload: (key: string) => void;
  onDelete: (key: string) => void;
  onCopyKey: (key: string) => void;
}) {
  return (
    <div className="rounded-lg border">
      <table className="w-full" data-testid="objects-table">
        <thead>
          <tr className="border-b bg-muted/50">
            <th className="px-4 py-2.5 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
              Key
            </th>
            <th className="px-4 py-2.5 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider w-28">
              Size
            </th>
            <th className="px-4 py-2.5 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider w-44">
              Last Modified
            </th>
            <th className="px-4 py-2.5 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider w-20">
              Actions
            </th>
          </tr>
        </thead>
        <tbody>
          {folders.map((folderName) => (
            <tr
              key={`folder-${folderName}`}
              data-testid={`entry-row-prefix-${folderName}`}
              className="border-b last:border-b-0 hover:bg-muted/30 transition-colors"
            >
              <td className="px-4 py-2.5">
                <button
                  className="flex items-center gap-2.5 hover:underline"
                  onClick={() => onNavigate(prefix + folderName)}
                >
                  <Folder className="h-4.5 w-4.5 flex-shrink-0 text-amber-500" />
                  <span className="text-sm font-medium">{folderName}</span>
                </button>
              </td>
              <td className="px-4 py-2.5 text-sm text-muted-foreground">--</td>
              <td className="px-4 py-2.5 text-sm text-muted-foreground">--</td>
              <td className="px-4 py-2.5" />
            </tr>
          ))}
          {files.map((obj) => {
            const displayKey = prefix ? obj.key.slice(prefix.length) : obj.key;
            return (
              <ContextMenu key={obj.key}>
                <ContextMenuTrigger asChild>
                  <tr
                    data-testid={`entry-row-object-${displayKey}`}
                    className="border-b last:border-b-0 hover:bg-muted/30 transition-colors"
                  >
                    <td className="px-4 py-2.5">
                      <span className="flex items-center gap-2.5">
                        <FileText className="h-4.5 w-4.5 flex-shrink-0 text-slate-400" />
                        <span className="text-sm">
                          {prefix && (
                            <span className="text-muted-foreground">{prefix}</span>
                          )}
                          {displayKey}
                        </span>
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-sm text-muted-foreground">
                      {formatBytes(obj.size)}
                    </td>
                    <td className="px-4 py-2.5 text-sm text-muted-foreground">
                      {obj.lastModified > 0 ? formatTimestamp(obj.lastModified) : "--"}
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <ObjectActions
                        objectKey={obj.key}
                        onDownload={onDownload}
                        onDelete={onDelete}
                        onCopyKey={onCopyKey}
                      />
                    </td>
                  </tr>
                </ContextMenuTrigger>
                <ContextMenuContent>
                  <ContextMenuItem onClick={() => onDownload(obj.key)}>
                    <Download className="mr-2 h-4 w-4" />
                    Download
                  </ContextMenuItem>
                  <ContextMenuItem onClick={() => onCopyKey(obj.key)}>
                    <Copy className="mr-2 h-4 w-4" />
                    Copy S3 URI
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem
                    onClick={() => onDelete(obj.key)}
                    className="text-destructive focus:text-destructive"
                  >
                    <Trash2 className="mr-2 h-4 w-4" />
                    Delete
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

// ── Grid View ─────────────────────────────────────────────────────────────────

function GridView({
  folders,
  files,
  prefix,
  onNavigate,
  onDownload,
  onDelete,
  onCopyKey,
}: {
  folders: string[];
  files: S3ObjectInfo[];
  prefix: string;
  onNavigate: (prefix: string) => void;
  onDownload: (key: string) => void;
  onDelete: (key: string) => void;
  onCopyKey: (key: string) => void;
}) {
  return (
    <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-3" data-testid="objects-grid">
      {folders.map((folderName) => (
        <div
          key={`folder-${folderName}`}
          className="flex flex-col items-center gap-2 rounded-lg border p-4 hover:bg-muted/50 transition-colors cursor-pointer"
          onClick={() => onNavigate(prefix + folderName)}
        >
          <Folder className="h-10 w-10 text-amber-500" />
          <p className="text-sm font-medium truncate w-full text-center">{folderName}</p>
        </div>
      ))}
      {files.map((obj) => {
        const displayKey = prefix ? obj.key.slice(prefix.length) : obj.key;
        return (
          <div
            key={obj.key}
            className="group relative flex flex-col items-center gap-2 rounded-lg border p-4 hover:bg-muted/50 transition-colors"
          >
            <FileText className="h-10 w-10 text-slate-400" />
            <div className="text-center min-w-0 w-full">
              <p className="text-sm font-medium truncate">{displayKey}</p>
              <p className="text-xs text-muted-foreground">{formatBytes(obj.size)}</p>
            </div>
            <div className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity">
              <ObjectActions
                objectKey={obj.key}
                onDownload={onDownload}
                onDelete={onDelete}
                onCopyKey={onCopyKey}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ── Object Actions ────────────────────────────────────────────────────────────

function ObjectActions({
  objectKey,
  onDownload,
  onDelete,
  onCopyKey,
}: {
  objectKey: string;
  onDownload: (key: string) => void;
  onDelete: (key: string) => void;
  onCopyKey: (key: string) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          data-testid={`object-actions-${objectKey}`}
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          onClick={(e) => e.stopPropagation()}
        >
          <MoreHorizontal className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => onDownload(objectKey)}>
          <Download className="mr-2 h-4 w-4" />
          Download
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => onCopyKey(objectKey)}>
          <Copy className="mr-2 h-4 w-4" />
          Copy S3 URI
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => onDelete(objectKey)}
          className="text-destructive focus:text-destructive"
        >
          <Trash2 className="mr-2 h-4 w-4" />
          Delete
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
