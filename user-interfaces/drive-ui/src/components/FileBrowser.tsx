import { useRef, useState, useCallback, type ReactNode } from "react";
import {
  Upload,
  FolderPlus,
  RefreshCw,
  ChevronRight,
  Folder,
  FileText,
  FileImage,
  FileVideo,
  FileAudio,
  FileArchive,
  FileCode,
  File as FileIcon,
  Download,
  Trash2,
  MoreHorizontal,
  LayoutGrid,
  LayoutList,
  Loader2,
  Shield,
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
  useSelectedDrive,
  useCurrentPath,
  useEntries,
  useDriveLoading,
  useUploading,
  useViewMode,
  setViewMode,
  navigateTo,
  refreshDirectory,
  uploadFiles,
  downloadFile,
  deleteEntry,
  createFolder,
} from "@/state";
import { formatBytes, formatTimestamp } from "@/lib/utils";
import { toast } from "@/components/ui/toaster";
import type { FsEntry } from "@/lib/drive-client";
import UploadZone from "./UploadZone";
import NewFolderDialog from "./NewFolderDialog";
import ConfirmDialog from "./ConfirmDialog";
import EmptyState from "./EmptyState";
import CheckpointPanel from "./CheckpointPanel";
import PendingChangesBanner from "./PendingChangesBanner";

function getFileIcon(name: string) {
  const ext = name.split(".").pop()?.toLowerCase() || "";
  const imageExts = ["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "ico"];
  const videoExts = ["mp4", "webm", "avi", "mov", "mkv"];
  const audioExts = ["mp3", "wav", "ogg", "flac", "aac"];
  const archiveExts = ["zip", "tar", "gz", "rar", "7z"];
  const codeExts = ["ts", "tsx", "js", "jsx", "rs", "py", "go", "java", "c", "cpp", "h", "css", "html", "json", "toml", "yaml", "yml"];

  if (imageExts.includes(ext)) return FileImage;
  if (videoExts.includes(ext)) return FileVideo;
  if (audioExts.includes(ext)) return FileAudio;
  if (archiveExts.includes(ext)) return FileArchive;
  if (codeExts.includes(ext)) return FileCode;
  if (["txt", "md", "pdf", "doc", "docx"].includes(ext)) return FileText;
  return FileIcon;
}

function getFileIconColor(name: string) {
  const ext = name.split(".").pop()?.toLowerCase() || "";
  const imageExts = ["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "ico"];
  const videoExts = ["mp4", "webm", "avi", "mov", "mkv"];
  const audioExts = ["mp3", "wav", "ogg", "flac", "aac"];
  const archiveExts = ["zip", "tar", "gz", "rar", "7z"];
  const codeExts = ["ts", "tsx", "js", "jsx", "rs", "py", "go", "java", "c", "cpp", "h", "css", "html", "json", "toml", "yaml", "yml"];

  if (imageExts.includes(ext)) return "text-pink-500";
  if (videoExts.includes(ext)) return "text-purple-500";
  if (audioExts.includes(ext)) return "text-green-500";
  if (archiveExts.includes(ext)) return "text-orange-500";
  if (codeExts.includes(ext)) return "text-cyan-500";
  if (["pdf"].includes(ext)) return "text-red-500";
  return "text-slate-400";
}

export default function FileBrowser() {
  const selectedDrive = useSelectedDrive();
  const currentPath = useCurrentPath();
  const entries = useEntries();
  const loading = useDriveLoading();
  const uploading = useUploading();
  const viewMode = useViewMode();

  const [showNewFolder, setShowNewFolder] = useState(false);
  const [entryToDelete, setEntryToDelete] = useState<FsEntry | null>(null);
  const [showCheckpoint, setShowCheckpoint] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const handleFileSelect = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const files = Array.from(e.target.files || []);
      if (files.length > 0) {
        try {
          await uploadFiles(files);
          toast({
            title: "Upload complete",
            description: `${files.length} file${files.length > 1 ? "s" : ""} uploaded`,
          });
        } catch (err) {
          if ((err as Error).name !== "AbortError") {
            toast({
              title: "Upload failed",
              description: err instanceof Error ? err.message : "Error",
              variant: "destructive",
            });
          }
        }
      }
      e.target.value = "";
    },
    [],
  );

  const handleDownload = useCallback(
    async (entry: FsEntry) => {
      try {
        const blob = await downloadFile(entry);
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
        toast({
          title: "Download failed",
          description: err instanceof Error ? err.message : "Error",
          variant: "destructive",
        });
      }
    },
    [],
  );

  const handleDelete = useCallback(
    async (entry: FsEntry) => {
      try {
        await deleteEntry(entry);
        toast({ title: "Deleted", description: entry.name });
      } catch (err) {
        toast({
          title: "Delete failed",
          description: err instanceof Error ? err.message : "Error",
          variant: "destructive",
        });
      }
    },
    [],
  );

  const handleCreateFolder = useCallback(async (name: string) => {
    try {
      await createFolder(name);
      toast({ title: "Folder created", description: name });
    } catch (err) {
      toast({
        title: "Create folder failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  }, []);

  if (!selectedDrive) return null;

  const breadcrumbs = () => {
    const driveName = selectedDrive.name || `Drive ${selectedDrive.driveId}`;
    const segments = [{ name: driveName, path: "/" }];
    if (currentPath !== "/") {
      const parts = currentPath.split("/").filter(Boolean);
      let acc = "";
      for (const part of parts) {
        acc += "/" + part;
        segments.push({ name: part, path: acc });
      }
    }
    return segments;
  };

  const directories = entries.filter((e) => e.entryType === "directory");
  const files = entries.filter((e) => e.entryType === "file");
  const sortedEntries = [...directories, ...files];

  return (
    <UploadZone>
      <div className="flex flex-col h-full" data-testid="file-browser">
        <div className="flex items-center justify-between gap-2 pb-4 border-b">
          <nav data-testid="breadcrumbs" className="flex items-center gap-1 text-sm min-w-0">
            {breadcrumbs().map((seg, i, arr) => (
              <span key={seg.path} className="flex items-center gap-1">
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
                  onClick={() => navigateTo(seg.path)}
                >
                  {seg.name}
                </button>
              </span>
            ))}
          </nav>

          <div className="flex items-center gap-1.5">
            <Button
              data-testid="upload-button"
              variant="outline"
              size="sm"
              onClick={() => fileInputRef.current?.click()}
              disabled={uploading}
            >
              {uploading ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Upload className="mr-2 h-4 w-4" />
              )}
              Upload
            </Button>
            <input
              ref={fileInputRef}
              type="file"
              multiple
              className="hidden"
              onChange={handleFileSelect}
            />
            <Button
              data-testid="new-folder-button"
              variant="outline"
              size="sm"
              onClick={() => setShowNewFolder(true)}
            >
              <FolderPlus className="mr-2 h-4 w-4" />
              New Folder
            </Button>
            <div className="w-px h-6 bg-border mx-1" />
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
              onClick={() => refreshDirectory()}
              disabled={loading}
            >
              <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
            </Button>
          </div>
        </div>

        <PendingChangesBanner />

        {showCheckpoint && (
          <div className="pt-4">
            <CheckpointPanel />
          </div>
        )}

        <div className="flex-1 pt-4">
          {loading && sortedEntries.length === 0 ? (
            <div className="flex items-center justify-center py-16">
              <RefreshCw className="h-6 w-6 animate-spin text-muted-foreground" />
            </div>
          ) : sortedEntries.length === 0 ? (
            <EmptyState
              icon={<Folder className="h-12 w-12" />}
              title="This folder is empty"
              description="Upload files or create folders to get started."
              action={
                <div className="flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => fileInputRef.current?.click()}
                  >
                    <Upload className="mr-2 h-4 w-4" />
                    Upload Files
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setShowNewFolder(true)}
                  >
                    <FolderPlus className="mr-2 h-4 w-4" />
                    New Folder
                  </Button>
                </div>
              }
            />
          ) : viewMode === "list" ? (
            <ListView
              entries={sortedEntries}
              onNavigate={navigateTo}
              onDownload={handleDownload}
              onDelete={setEntryToDelete}
            />
          ) : (
            <GridView
              entries={sortedEntries}
              onNavigate={navigateTo}
              onDownload={handleDownload}
              onDelete={setEntryToDelete}
            />
          )}
        </div>
      </div>

      <NewFolderDialog
        open={showNewFolder}
        onOpenChange={setShowNewFolder}
        onConfirm={handleCreateFolder}
      />

      <ConfirmDialog
        open={!!entryToDelete}
        onOpenChange={() => setEntryToDelete(null)}
        title={`Delete "${entryToDelete?.name}"?`}
        description={
          entryToDelete?.entryType === "directory"
            ? "This will delete the folder entry. Files inside may remain in storage."
            : "This file will be removed from the directory listing."
        }
        onConfirm={() => {
          if (entryToDelete) handleDelete(entryToDelete);
        }}
      />
    </UploadZone>
  );
}

function ListView({
  entries,
  onNavigate,
  onDownload,
  onDelete,
}: {
  entries: FsEntry[];
  onNavigate: (path: string) => void;
  onDownload: (entry: FsEntry) => void;
  onDelete: (entry: FsEntry) => void;
}) {
  return (
    <div className="rounded-lg border">
      <table className="w-full" data-testid="entries-table">
        <thead>
          <tr className="border-b bg-muted/50">
            <th className="px-4 py-2.5 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
              Name
            </th>
            <th className="px-4 py-2.5 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider w-28">
              Size
            </th>
            <th className="px-4 py-2.5 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider w-44">
              Modified
            </th>
            <th className="px-4 py-2.5 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider w-20">
              Actions
            </th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry) => {
            const isDir = entry.entryType === "directory";
            const Icon = isDir ? Folder : getFileIcon(entry.name);
            const iconColor = isDir ? "text-amber-500" : getFileIconColor(entry.name);

            return (
              <EntryContextMenu
                key={entry.path}
                entry={entry}
                onNavigate={onNavigate}
                onDownload={onDownload}
                onDelete={onDelete}
              >
                <tr
                  data-testid={`entry-row-${entry.entryType}-${entry.name}`}
                  className="border-b last:border-b-0 hover:bg-muted/30 transition-colors"
                >
                  <td className="px-4 py-2.5">
                    {isDir ? (
                      <button
                        className="flex items-center gap-2.5 hover:underline"
                        onClick={() => onNavigate(entry.path)}
                      >
                        <Icon className={`h-4.5 w-4.5 flex-shrink-0 ${iconColor}`} />
                        <span className="text-sm font-medium">{entry.name}</span>
                      </button>
                    ) : (
                      <span className="flex items-center gap-2.5">
                        <Icon className={`h-4.5 w-4.5 flex-shrink-0 ${iconColor}`} />
                        <span className="text-sm">{entry.name}</span>
                      </span>
                    )}
                  </td>
                  <td className="px-4 py-2.5 text-sm text-muted-foreground">
                    {isDir ? "--" : formatBytes(entry.size)}
                  </td>
                  <td className="px-4 py-2.5 text-sm text-muted-foreground">
                    {entry.mtime > 0 ? formatTimestamp(entry.mtime) : "--"}
                  </td>
                  <td className="px-4 py-2.5 text-right">
                    <EntryActions entry={entry} onDownload={onDownload} onDelete={onDelete} />
                  </td>
                </tr>
              </EntryContextMenu>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function GridView({
  entries,
  onNavigate,
  onDownload,
  onDelete,
}: {
  entries: FsEntry[];
  onNavigate: (path: string) => void;
  onDownload: (entry: FsEntry) => void;
  onDelete: (entry: FsEntry) => void;
}) {
  return (
    <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-3" data-testid="entries-grid">
      {entries.map((entry) => {
        const isDir = entry.entryType === "directory";
        const Icon = isDir ? Folder : getFileIcon(entry.name);
        const iconColor = isDir ? "text-amber-500" : getFileIconColor(entry.name);

        return (
          <EntryContextMenu
            key={entry.path}
            entry={entry}
            onNavigate={onNavigate}
            onDownload={onDownload}
            onDelete={onDelete}
          >
            <div
              data-testid={`entry-card-${entry.entryType}-${entry.name}`}
              className="group relative flex flex-col items-center gap-2 rounded-lg border p-4 hover:bg-muted/50 transition-colors cursor-pointer"
              onClick={() => {
                if (isDir) onNavigate(entry.path);
              }}
            >
              <Icon className={`h-10 w-10 ${iconColor}`} />
              <div className="text-center min-w-0 w-full">
                <p className="text-sm font-medium truncate">{entry.name}</p>
                {!isDir && (
                  <p className="text-xs text-muted-foreground">
                    {formatBytes(entry.size)}
                  </p>
                )}
              </div>
              <div className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity">
                <EntryActions entry={entry} onDownload={onDownload} onDelete={onDelete} />
              </div>
            </div>
          </EntryContextMenu>
        );
      })}
    </div>
  );
}

function EntryActions({
  entry,
  onDownload,
  onDelete,
}: {
  entry: FsEntry;
  onDownload: (entry: FsEntry) => void;
  onDelete: (entry: FsEntry) => void;
}) {
  const isFile = entry.entryType === "file";

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          data-testid={`entry-actions-${entry.name}`}
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          onClick={(e) => e.stopPropagation()}
        >
          <MoreHorizontal className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {isFile && (
          <DropdownMenuItem
            onClick={(e) => {
              e.stopPropagation();
              onDownload(entry);
            }}
          >
            <Download className="mr-2 h-4 w-4" />
            Download
          </DropdownMenuItem>
        )}
        {isFile && <DropdownMenuSeparator />}
        <DropdownMenuItem
          onClick={(e) => {
            e.stopPropagation();
            onDelete(entry);
          }}
          className="text-destructive focus:text-destructive"
        >
          <Trash2 className="mr-2 h-4 w-4" />
          Delete
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function EntryContextMenu({
  entry,
  onNavigate,
  onDownload,
  onDelete,
  children,
}: {
  entry: FsEntry;
  onNavigate: (path: string) => void;
  onDownload: (entry: FsEntry) => void;
  onDelete: (entry: FsEntry) => void;
  children: ReactNode;
}) {
  const isDir = entry.entryType === "directory";

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        {isDir && (
          <ContextMenuItem onClick={() => onNavigate(entry.path)}>
            <Folder className="mr-2 h-4 w-4" />
            Open
          </ContextMenuItem>
        )}
        {!isDir && (
          <ContextMenuItem onClick={() => onDownload(entry)}>
            <Download className="mr-2 h-4 w-4" />
            Download
          </ContextMenuItem>
        )}
        <ContextMenuSeparator />
        <ContextMenuItem
          onClick={() => onDelete(entry)}
          className="text-destructive focus:text-destructive"
        >
          <Trash2 className="mr-2 h-4 w-4" />
          Delete
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
