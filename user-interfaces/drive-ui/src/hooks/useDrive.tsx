import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  type ReactNode,
} from "react";
import { useChain } from "./useChain";
import {
  DriveClient,
  getDriveClient,
  type DriveInfo,
  type FsEntry,
  type CreateDriveOptions,
  type BucketMember,
  type MemberRole,
  type CheckpointInfo,
  type CheckpointDuty,
} from "@/lib/drive-client";
import { toast } from "@/components/ui/toaster";

// ─────────────────────────────────────────────────────────────────────────────
// Creation status tracking
// ─────────────────────────────────────────────────────────────────────────────

export type CreationStage =
  | "submitting"
  | "created"
  | "waiting"
  | "ready"
  | "failed";

export interface CreationStatus {
  id: string;
  name: string;
  stage: CreationStage;
  statusMessage?: string;
  elapsedMs: number;
  error?: string;
  bucketId?: bigint;
}

// ─────────────────────────────────────────────────────────────────────────────
// Context
// ─────────────────────────────────────────────────────────────────────────────

interface DriveState {
  client: DriveClient | null;
  signerAddress: string | null;
  signerName: string | null;
  drives: DriveInfo[];
  selectedDrive: DriveInfo | null;
  currentPath: string;
  entries: FsEntry[];
  loading: boolean;
  uploading: boolean;
  error: string | null;
  balance: { free: bigint; reserved: bigint } | null;
  viewMode: "list" | "grid";
  creations: CreationStatus[];

  setSigner: (seed: string, name?: string) => Promise<void>;
  createDrive: (options: CreateDriveOptions) => Promise<void>;
  refreshDrives: () => Promise<void>;
  deleteDrive: (driveId: bigint) => Promise<void>;
  selectDrive: (drive: DriveInfo | null) => void;
  navigateTo: (path: string) => void;
  navigateUp: () => void;
  refreshDirectory: () => Promise<void>;
  uploadFiles: (files: File[]) => Promise<void>;
  downloadFile: (entry: FsEntry) => Promise<void>;
  deleteEntry: (entry: FsEntry) => Promise<void>;
  createFolder: (name: string) => Promise<void>;
  setViewMode: (mode: "list" | "grid") => void;
  dismissCreation: (id: string) => void;

  // Members
  fetchMembers: (bucketId: bigint) => Promise<BucketMember[]>;
  addMember: (bucketId: bigint, account: string, role: MemberRole) => Promise<void>;
  removeMember: (bucketId: bigint, account: string) => Promise<void>;

  // Checkpoint
  checkpointInfo: CheckpointInfo | null;
  checkpointDuty: CheckpointDuty | null;
  checkpointLoading: boolean;
  refreshCheckpoint: () => Promise<void>;
  triggerCheckpoint: () => Promise<void>;
}

const DriveContext = createContext<DriveState | null>(null);

export function DriveProvider({ children }: { children: ReactNode }) {
  const { connected, chainEndpoint } = useChain();

  const [client, setClient] = useState<DriveClient | null>(null);
  const [signerAddress, setSignerAddress] = useState<string | null>(null);
  const [signerName, setSignerName] = useState<string | null>(null);
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [selectedDrive, setSelectedDrive] = useState<DriveInfo | null>(null);
  const [currentPath, setCurrentPath] = useState("/");
  const [entries, setEntries] = useState<FsEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [balance, setBalance] = useState<{ free: bigint; reserved: bigint } | null>(null);
  const [viewMode, setViewMode] = useState<"list" | "grid">("list");
  const [creations, setCreations] = useState<CreationStatus[]>([]);
  const [checkpointInfo, setCheckpointInfo] = useState<CheckpointInfo | null>(null);
  const [checkpointDuty, setCheckpointDuty] = useState<CheckpointDuty | null>(null);
  const [checkpointLoading, setCheckpointLoading] = useState(false);

  // Initialize client when connected
  useEffect(() => {
    if (connected && chainEndpoint) {
      const c = getDriveClient(chainEndpoint);
      if (!c.isConnected()) {
        c.connect().then(() => setClient(c)).catch(console.error);
      } else {
        setClient(c);
      }
    }
  }, [connected, chainEndpoint]);

  // Fetch balance when signer changes
  useEffect(() => {
    if (client?.isConnected() && signerAddress) {
      client.getBalance(signerAddress).then(setBalance).catch(console.error);
    }
  }, [client, signerAddress]);

  // Refresh drives when signer changes
  useEffect(() => {
    if (client?.isConnected() && client.hasSigner()) {
      refreshDrives();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, signerAddress]);

  // Refresh directory when selected drive or path changes
  useEffect(() => {
    if (selectedDrive && client?.isConnected()) {
      let cancelled = false;
      setLoading(true);
      setError(null);
      client
        .listDirectory(selectedDrive.bucketId, currentPath)
        .then((dirEntries) => {
          if (!cancelled) setEntries(dirEntries);
        })
        .catch((err) => {
          if (!cancelled) {
            setError(err instanceof Error ? err.message : "Failed to list directory");
            setEntries([]);
          }
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
      return () => { cancelled = true; };
    } else {
      setEntries([]);
    }
  }, [selectedDrive, currentPath, client]);

  const setSigner = useCallback(
    async (seed: string, name?: string) => {
      if (!client) return;
      const addr = await client.setSigner(seed);
      setSignerAddress(addr);
      setSignerName(name ?? null);
      setSelectedDrive(null);
      setCurrentPath("/");
      setEntries([]);
    },
    [client]
  );

  const refreshDrives = useCallback(async () => {
    if (!client?.isConnected() || !client.hasSigner()) return;
    setLoading(true);
    try {
      const driveList = await client.listDrives();
      setDrives(driveList);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load drives");
    } finally {
      setLoading(false);
    }
  }, [client]);

  const refreshDirectory = useCallback(async () => {
    if (!client || !selectedDrive) return;
    setLoading(true);
    setError(null);
    try {
      const dirEntries = await client.listDirectory(selectedDrive.bucketId, currentPath);
      setEntries(dirEntries);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to list directory");
      setEntries([]);
    } finally {
      setLoading(false);
    }
  }, [client, selectedDrive, currentPath]);

  const updateCreation = useCallback((id: string, updates: Partial<CreationStatus>) => {
    setCreations((prev) => prev.map((c) => (c.id === id ? { ...c, ...updates } : c)));
  }, []);

  const dismissCreation = useCallback((id: string) => {
    setCreations((prev) => prev.filter((c) => c.id !== id));
  }, []);

  const selectDrive = useCallback(
    async (drive: DriveInfo | null) => {
      setSelectedDrive(drive);
      setCurrentPath("/");
      if (drive && client?.isConnected()) {
        setLoading(true);
        setError(null);
        try {
          const dirEntries = await client.listDirectory(drive.bucketId, "/");
          setEntries(dirEntries);
        } catch {
          setEntries([]);
        } finally {
          setLoading(false);
        }
      } else {
        setEntries([]);
      }
    },
    [client]
  );

  const createDrive = useCallback(
    async (options: CreateDriveOptions) => {
      if (!client) return;

      const creationId = crypto.randomUUID();
      const name = options.name || "Untitled Drive";

      setCreations((prev) => [
        ...prev,
        {
          id: creationId,
          name,
          stage: "submitting",
          elapsedMs: 0,
        },
      ]);

      try {
        const drive = await client.createDrive(options);
        updateCreation(creationId, {
          stage: "created",
          bucketId: drive.bucketId,
        });

        // Wait for provider in background
        updateCreation(creationId, { stage: "waiting" });
        try {
          await client.waitForProvider(drive.bucketId, (status, elapsedMs) => {
            updateCreation(creationId, { statusMessage: status, elapsedMs });
          });
          updateCreation(creationId, { stage: "ready" });
          await refreshDrives();
          await selectDrive(drive);
          toast({ title: "Drive created", description: `"${name}" is ready to use` });
        } catch (err) {
          updateCreation(creationId, {
            stage: "failed",
            error: err instanceof Error ? err.message : "Provider did not accept",
          });
          // Still refresh drives - it was created on chain
          await refreshDrives();
        }
      } catch (err) {
        updateCreation(creationId, {
          stage: "failed",
          error: err instanceof Error ? err.message : "Failed to create drive",
        });
      }
    },
    [client, refreshDrives, updateCreation, selectDrive]
  );

  const deleteDrive = useCallback(
    async (driveId: bigint) => {
      if (!client) return;
      try {
        await client.deleteDrive(driveId);
        if (selectedDrive?.driveId === driveId) {
          setSelectedDrive(null);
          setEntries([]);
          setCurrentPath("/");
        }
        await refreshDrives();
        toast({ title: "Drive deleted" });
      } catch (err) {
        toast({
          title: "Delete failed",
          description: err instanceof Error ? err.message : "Error",
          variant: "destructive",
        });
      }
    },
    [client, selectedDrive, refreshDrives]
  );

  const navigateTo = useCallback((path: string) => {
    setCurrentPath(path);
  }, []);

  const navigateUp = useCallback(() => {
    if (currentPath === "/") return;
    const parts = currentPath.split("/").filter(Boolean);
    parts.pop();
    setCurrentPath(parts.length === 0 ? "/" : "/" + parts.join("/"));
  }, [currentPath]);

  const readFileAsUint8Array = (file: File): Promise<Uint8Array> => {
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

  const uploadFiles = useCallback(
    async (files: File[]) => {
      if (!client || !selectedDrive) return;
      setUploading(true);
      try {
        for (const file of files) {
          const data = await readFileAsUint8Array(file);
          const filePath =
            currentPath === "/"
              ? `/${file.name}`
              : `${currentPath}/${file.name}`;
          await client.uploadFile(
            selectedDrive.bucketId,
            filePath,
            data,
            file.type || "application/octet-stream"
          );
        }
        toast({
          title: "Upload complete",
          description: `${files.length} file${files.length > 1 ? "s" : ""} uploaded`,
        });
        await refreshDirectory();
      } catch (err) {
        toast({
          title: "Upload failed",
          description: err instanceof Error ? err.message : "Error",
          variant: "destructive",
        });
      } finally {
        setUploading(false);
      }
    },
    [client, selectedDrive, currentPath, refreshDirectory]
  );

  const downloadFile = useCallback(
    async (entry: FsEntry) => {
      if (!client || !selectedDrive) return;
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
        toast({
          title: "Download failed",
          description: err instanceof Error ? err.message : "Error",
          variant: "destructive",
        });
      }
    },
    [client, selectedDrive]
  );

  const deleteEntry = useCallback(
    async (entry: FsEntry) => {
      if (!client || !selectedDrive) return;
      try {
        await client.deleteFile(selectedDrive.bucketId, entry.path);
        toast({ title: "Deleted", description: entry.name });
        await refreshDirectory();
      } catch (err) {
        toast({
          title: "Delete failed",
          description: err instanceof Error ? err.message : "Error",
          variant: "destructive",
        });
      }
    },
    [client, selectedDrive, refreshDirectory]
  );

  const createFolder = useCallback(
    async (name: string) => {
      if (!client || !selectedDrive) return;
      try {
        const folderPath =
          currentPath === "/" ? `/${name}` : `${currentPath}/${name}`;
        await client.createDirectory(selectedDrive.bucketId, folderPath);
        toast({ title: "Folder created", description: name });
        await refreshDirectory();
      } catch (err) {
        toast({
          title: "Create folder failed",
          description: err instanceof Error ? err.message : "Error",
          variant: "destructive",
        });
      }
    },
    [client, selectedDrive, currentPath, refreshDirectory]
  );

  // ── Member methods ──────────────────────────────────────────────────────

  const fetchMembers = useCallback(
    async (bucketId: bigint): Promise<BucketMember[]> => {
      if (!client) return [];
      return client.getBucketMembers(bucketId);
    },
    [client]
  );

  const addMember = useCallback(
    async (bucketId: bigint, account: string, role: MemberRole) => {
      if (!client) return;
      await client.addMember(bucketId, account, role);
    },
    [client]
  );

  const removeMember = useCallback(
    async (bucketId: bigint, account: string) => {
      if (!client) return;
      await client.removeMember(bucketId, account);
    },
    [client]
  );

  // ── Checkpoint methods ──────────────────────────────────────────────────

  const refreshCheckpoint = useCallback(async () => {
    if (!client || !selectedDrive) return;
    setCheckpointLoading(true);
    try {
      const [info, duty] = await Promise.all([
        client.getCheckpointInfo(selectedDrive.bucketId).catch(() => null),
        client.getCheckpointDuty(selectedDrive.bucketId).catch(() => null),
      ]);
      setCheckpointInfo(info);
      setCheckpointDuty(duty);
    } finally {
      setCheckpointLoading(false);
    }
  }, [client, selectedDrive]);

  const triggerCheckpointAction = useCallback(async () => {
    if (!client || !selectedDrive) return;
    try {
      await client.triggerCheckpoint(selectedDrive.bucketId);
      toast({ title: "Checkpoint triggered", description: "Provider is creating a checkpoint..." });
      // Delayed refresh to allow provider to process
      setTimeout(() => refreshCheckpoint(), 5000);
    } catch (err) {
      toast({
        title: "Checkpoint failed",
        description: err instanceof Error ? err.message : "Error",
        variant: "destructive",
      });
    }
  }, [client, selectedDrive, refreshCheckpoint]);

  // Auto-refresh checkpoint when selectedDrive changes
  useEffect(() => {
    if (selectedDrive && client?.isConnected()) {
      refreshCheckpoint();
    } else {
      setCheckpointInfo(null);
      setCheckpointDuty(null);
    }
  }, [selectedDrive, client, refreshCheckpoint]);

  return (
    <DriveContext.Provider
      value={{
        client,
        signerAddress,
        signerName,
        drives,
        selectedDrive,
        currentPath,
        entries,
        loading,
        uploading,
        error,
        balance,
        viewMode,
        creations,
        setSigner,
        createDrive,
        refreshDrives,
        deleteDrive,
        selectDrive,
        navigateTo,
        navigateUp,
        refreshDirectory,
        uploadFiles,
        downloadFile,
        deleteEntry,
        createFolder,
        setViewMode,
        dismissCreation,
        fetchMembers,
        addMember,
        removeMember,
        checkpointInfo,
        checkpointDuty,
        checkpointLoading,
        refreshCheckpoint,
        triggerCheckpoint: triggerCheckpointAction,
      }}
    >
      {children}
    </DriveContext.Provider>
  );
}

export function useDrive() {
  const context = useContext(DriveContext);
  if (!context) {
    throw new Error("useDrive must be used within a DriveProvider");
  }
  return context;
}
