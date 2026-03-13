import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  type ReactNode,
} from "react";
import {
  StorageClient,
  type DriveInfo,
  type BucketInfo,
  type BucketMember,
  type ProviderEndpointInfo,
  type CreateDriveOptions,
  type CreateBucketOptions,
  type UploadResult,
  type PutObjectOptions,
  type S3ObjectInfo,
  type FsEntryInfo,
  type CheckpointStatus,
} from "@/lib/storage";
import { useChain } from "./useChain";

interface StorageState {
  client: StorageClient | null;
  signerAddress: string | null;
  signerName: string | null;
  drives: DriveInfo[];
  buckets: BucketInfo[];
  loading: boolean;
  error: string | null;
  balance: { free: bigint; reserved: bigint } | null;
  providerCapacity: { max: bigint; used: bigint } | null;

  // Account
  setSigner: (seed: string) => Promise<void>;

  // Drives (File System)
  createDrive: (options: CreateDriveOptions) => Promise<bigint>;
  refreshDrives: () => Promise<void>;
  deleteDrive: (driveId: bigint) => Promise<void>;
  uploadToDrive: (driveId: bigint, bucketId: bigint, path: string, data: Uint8Array) => Promise<UploadResult>;
  downloadFromDrive: (bucketId: bigint, cid: string) => Promise<Uint8Array>;
  listDriveFiles: (bucketId: bigint, path?: string) => Promise<FsEntryInfo[]>;
  createDirectory: (bucketId: bigint, path: string) => Promise<void>;
  deleteFile: (bucketId: bigint, path: string) => Promise<void>;

  // Buckets (S3)
  createBucket: (name: string, options: CreateBucketOptions) => Promise<BucketInfo>;
  refreshBuckets: () => Promise<void>;
  deleteBucket: (name: string) => Promise<void>;
  putObject: (bucketName: string, key: string, data: Uint8Array, bucketId: bigint, options?: PutObjectOptions) => Promise<UploadResult>;
  getObject: (bucketId: bigint, cid: string) => Promise<Uint8Array>;
  listObjects: (bucketId: bigint, prefix?: string) => Promise<S3ObjectInfo[]>;
  deleteObject: (bucketId: bigint, key: string) => Promise<void>;

  // Provider
  checkProviderHealth: (bucketId: bigint) => Promise<boolean>;
  waitForProvider: (bucketId: bigint, onProgress?: (status: string, attempt: number, total: number) => void) => Promise<string>;

  // Bucket Members & Permissions
  fetchBucketMembers: (bucketId: bigint) => Promise<BucketMember[]>;
  addBucketMember: (bucketId: bigint, account: string, role: 'Admin' | 'Writer' | 'Reader') => Promise<void>;
  removeBucketMember: (bucketId: bigint, account: string) => Promise<void>;
  fetchBucketProviders: (bucketId: bigint) => Promise<ProviderEndpointInfo[]>;

  // Checkpoints
  fetchCheckpointStatus: (bucketId: bigint) => Promise<CheckpointStatus | null>;
  submitCheckpoint: (bucketId: bigint) => Promise<void>;
  configureCheckpointWindow: (bucketId: bigint, interval: number, gracePeriod: number, enabled: boolean) => Promise<void>;
  fundCheckpointPool: (bucketId: bigint, amount: bigint) => Promise<void>;
  claimCheckpointRewards: (bucketId: bigint) => Promise<void>;
}

const StorageContext = createContext<StorageState | null>(null);

export function StorageProvider({ children }: { children: ReactNode }) {
  const { connected, chainEndpoint, blockNumber } = useChain();
  const [client, setClient] = useState<StorageClient | null>(null);
  const [signerAddress, setSignerAddress] = useState<string | null>(null);
  const [signerName, setSignerName] = useState<string | null>(null);
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [buckets, setBuckets] = useState<BucketInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [balance, setBalance] = useState<{ free: bigint; reserved: bigint } | null>(null);
  const [providerCapacity, setProviderCapacity] = useState<{ max: bigint; used: bigint } | null>(null);

  // Initialize client when chain is connected
  useEffect(() => {
    if (connected && chainEndpoint) {
      console.log("[useStorage] Initializing client:", { chainEndpoint });
      const newClient = new StorageClient(chainEndpoint);
      newClient.connect().then(() => {
        console.log("[useStorage] Client connected successfully");
        setClient(newClient);
      }).catch((err) => {
        console.error("[useStorage] Client connection failed:", err);
        setError(err instanceof Error ? err.message : "Failed to connect storage client");
      });
    } else {
      if (client) {
        client.disconnect();
      }
      setClient(null);
      setSignerAddress(null);
      setSignerName(null);
      setDrives([]);
      setBuckets([]);
    }
  }, [connected, chainEndpoint]);

  const setSigner = useCallback(async (seed: string) => {
    if (!client) throw new Error("Client not connected");
    setLoading(true);
    setError(null);
    try {
      const address = await client.setSigner(seed);
      setSignerAddress(address);
      // Derive display name from well-known seeds
      const knownNames: Record<string, string> = {
        "//Alice": "Alice",
        "//Bob": "Bob",
        "//Charlie": "Charlie",
        "//Dave": "Dave",
        "//Eve": "Eve",
        "//Ferdie": "Ferdie",
      };
      setSignerName(knownNames[seed] || null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to set signer");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client]);

  // Fetch balance when signer changes
  useEffect(() => {
    if (client && signerAddress) {
      client.getBalance(signerAddress).then(setBalance).catch(() => setBalance(null));
    } else {
      setBalance(null);
    }
  }, [client, signerAddress]);

  // Fetch provider capacity when client connects
  useEffect(() => {
    if (client) {
      client.getProviderInfo().then((info) => {
        if (info) setProviderCapacity({ max: info.maxCapacity, used: info.committedBytes });
        else setProviderCapacity(null);
      }).catch(() => setProviderCapacity(null));
    } else {
      setProviderCapacity(null);
    }
  }, [client]);

  // --- Drive Operations ---

  const createDrive = useCallback(async (options: CreateDriveOptions): Promise<bigint> => {
    if (!client) throw new Error("Client not connected");
    if (!signerAddress) throw new Error("Signer not set");

    setLoading(true);
    setError(null);
    try {
      const driveId = await client.createDrive(options);
      // Skip refreshDrives — caller already has the driveId from events.
      // Refresh happens on next page visit via useEffect.
      return driveId;
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create drive");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client, signerAddress]);

  const refreshDrives = useCallback(async () => {
    if (!client) return;
    setLoading(true);
    try {
      const driveList = await client.listDrives();
      setDrives(driveList);
    } catch (err) {
      console.error("Failed to refresh drives:", err);
      const msg = err instanceof Error ? err.message : String(err);
      if (msg.includes("incompatible") || msg.includes("decode") || msg.includes("runtime")) {
        setError("Failed to list drives — runtime descriptor mismatch. Run: cd user-interfaces/console-ui && npx papi update");
      }
    } finally {
      setLoading(false);
    }
  }, [client]);

  const deleteDrive = useCallback(async (driveId: bigint) => {
    if (!client) throw new Error("Client not connected");

    setLoading(true);
    setError(null);
    try {
      await client.deleteDrive(driveId);
      await refreshDrives();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to delete drive");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client, refreshDrives]);

  const uploadToDrive = useCallback(async (
    driveId: bigint,
    bucketId: bigint,
    path: string,
    data: Uint8Array
  ): Promise<UploadResult> => {
    if (!client) throw new Error("Client not connected");

    setLoading(true);
    setError(null);
    try {
      return await client.uploadToDrive(driveId, bucketId, path, data);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Upload failed");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client]);

  const downloadFromDrive = useCallback(async (bucketId: bigint, cid: string): Promise<Uint8Array> => {
    if (!client) throw new Error("Client not connected");

    setLoading(true);
    setError(null);
    try {
      return await client.downloadByCid(bucketId, cid);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Download failed");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client]);

  const listDriveFiles = useCallback(async (bucketId: bigint, path?: string): Promise<FsEntryInfo[]> => {
    if (!client) throw new Error("Client not connected");
    return client.listDriveFiles(bucketId, path);
  }, [client]);

  // --- Bucket Operations ---

  const createBucket = useCallback(async (name: string, options: CreateBucketOptions): Promise<BucketInfo> => {
    if (!client) throw new Error("Client not connected");
    if (!signerAddress) throw new Error("Signer not set");

    setLoading(true);
    setError(null);
    try {
      console.log("[useStorage] createBucket:", name, options);
      const bucket = await client.createBucket(name, options);
      console.log("[useStorage] createBucket success:", bucket);
      // Skip refreshBuckets — caller already has BucketInfo from events.
      // Add to local state directly for immediate UI update.
      setBuckets((prev) => [...prev, bucket]);
      return bucket;
    } catch (err) {
      console.error("[useStorage] createBucket failed:", err);
      const msg = err instanceof Error ? err.message : String(err);
      console.error("[useStorage] Error message:", msg);
      if (msg.includes("incompatible") || msg.includes("runtime")) {
        console.error("[useStorage] HINT: Runtime descriptor mismatch. The chain runtime may have changed.");
        console.error("[useStorage] Fix: cd user-interfaces/console-ui && npx papi update");
      }
      setError(msg || "Failed to create bucket");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client, signerAddress]);

  const refreshBuckets = useCallback(async () => {
    if (!client) return;
    setLoading(true);
    try {
      const bucketList = await client.listBuckets();
      setBuckets(bucketList);
    } catch (err) {
      console.error("Failed to refresh buckets:", err);
      const msg = err instanceof Error ? err.message : String(err);
      if (msg.includes("incompatible") || msg.includes("decode") || msg.includes("runtime")) {
        setError("Failed to list buckets — runtime descriptor mismatch. Run: cd user-interfaces/console-ui && npx papi update");
      }
    } finally {
      setLoading(false);
    }
  }, [client]);

  const deleteBucket = useCallback(async (name: string) => {
    if (!client) throw new Error("Client not connected");

    setLoading(true);
    setError(null);
    try {
      await client.deleteBucket(name);
      await refreshBuckets();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to delete bucket");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client, refreshBuckets]);

  const putObject = useCallback(async (
    bucketName: string,
    key: string,
    data: Uint8Array,
    bucketId: bigint,
    options?: PutObjectOptions
  ): Promise<UploadResult> => {
    if (!client) throw new Error("Client not connected");

    setLoading(true);
    setError(null);
    try {
      return await client.putObject(bucketName, key, data, bucketId, options);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Upload failed");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client]);

  const getObject = useCallback(async (bucketId: bigint, cid: string): Promise<Uint8Array> => {
    if (!client) throw new Error("Client not connected");

    setLoading(true);
    setError(null);
    try {
      return await client.getObject(bucketId, cid);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Download failed");
      throw err;
    } finally {
      setLoading(false);
    }
  }, [client]);

  const listObjects = useCallback(async (bucketId: bigint, prefix?: string): Promise<S3ObjectInfo[]> => {
    if (!client) throw new Error("Client not connected");
    return client.listObjects(bucketId, prefix);
  }, [client]);

  // --- Additional FS/S3 Operations ---

  const createDirectory = useCallback(async (bucketId: bigint, path: string): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    await client.createDirectory(bucketId, path);
  }, [client]);

  const deleteFile = useCallback(async (bucketId: bigint, path: string): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    await client.deleteFile(bucketId, path);
  }, [client]);

  const deleteObject = useCallback(async (bucketId: bigint, key: string): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    await client.deleteObject(bucketId, key);
  }, [client]);

  // --- Provider Health ---

  const checkProviderHealth = useCallback(async (bucketId: bigint): Promise<boolean> => {
    if (!client) return false;
    return client.checkProviderHealth(bucketId);
  }, [client]);

  const waitForProvider = useCallback(async (
    bucketId: bigint,
    onProgress?: (status: string, attempt: number, total: number) => void,
  ): Promise<string> => {
    if (!client) throw new Error("Client not connected");
    return client.waitForProvider(bucketId, onProgress);
  }, [client]);

  // --- Bucket Members & Permissions ---

  const fetchBucketMembers = useCallback(async (bucketId: bigint): Promise<BucketMember[]> => {
    if (!client) throw new Error("Client not connected");
    return client.getBucketMembers(bucketId);
  }, [client]);

  const addBucketMember = useCallback(async (
    bucketId: bigint, account: string, role: 'Admin' | 'Writer' | 'Reader'
  ): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    setError(null);
    try {
      await client.setMember(bucketId, account, role);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to add member";
      setError(msg);
      throw err;
    }
  }, [client]);

  const removeBucketMember = useCallback(async (bucketId: bigint, account: string): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    setError(null);
    try {
      await client.removeMember(bucketId, account);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to remove member";
      setError(msg);
      throw err;
    }
  }, [client]);

  const fetchBucketProviders = useCallback(async (bucketId: bigint): Promise<ProviderEndpointInfo[]> => {
    if (!client) throw new Error("Client not connected");
    return client.getBucketProviders(bucketId);
  }, [client]);

  // --- Checkpoint Operations ---

  const fetchCheckpointStatus = useCallback(async (bucketId: bigint): Promise<CheckpointStatus | null> => {
    if (!client) return null;
    try {
      return await client.getCheckpointStatus(bucketId, blockNumber);
    } catch (err) {
      console.error("Failed to fetch checkpoint status:", err);
      return null;
    }
  }, [client, blockNumber]);

  const submitCheckpoint = useCallback(async (bucketId: bigint): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    setError(null);
    try {
      await client.submitCheckpointForBucket(bucketId, blockNumber);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to submit checkpoint";
      setError(msg);
      throw err;
    }
  }, [client, blockNumber]);

  const configureCheckpointWindow = useCallback(async (
    bucketId: bigint, interval: number, gracePeriod: number, enabled: boolean
  ): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    setError(null);
    try {
      await client.configureCheckpointWindow(bucketId, interval, gracePeriod, enabled);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to configure checkpoint";
      setError(msg);
      throw err;
    }
  }, [client]);

  const fundCheckpointPool = useCallback(async (bucketId: bigint, amount: bigint): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    setError(null);
    try {
      await client.fundCheckpointPool(bucketId, amount);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to fund checkpoint pool";
      setError(msg);
      throw err;
    }
  }, [client]);

  const claimCheckpointRewards = useCallback(async (bucketId: bigint): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    setError(null);
    try {
      await client.claimCheckpointRewards(bucketId);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to claim checkpoint rewards";
      setError(msg);
      throw err;
    }
  }, [client]);

  return (
    <StorageContext.Provider
      value={{
        client,
        signerAddress,
        signerName,
        drives,
        buckets,
        loading,
        error,
        balance,
        providerCapacity,
        setSigner,
        createDrive,
        refreshDrives,
        deleteDrive,
        uploadToDrive,
        downloadFromDrive,
        listDriveFiles,
        createDirectory,
        deleteFile,
        createBucket,
        refreshBuckets,
        deleteBucket,
        putObject,
        getObject,
        listObjects,
        deleteObject,
        checkProviderHealth,
        waitForProvider,
        fetchBucketMembers,
        addBucketMember,
        removeBucketMember,
        fetchBucketProviders,
        fetchCheckpointStatus,
        submitCheckpoint,
        configureCheckpointWindow,
        fundCheckpointPool,
        claimCheckpointRewards,
      }}
    >
      {children}
    </StorageContext.Provider>
  );
}

export function useStorage() {
  const context = useContext(StorageContext);
  if (!context) {
    throw new Error("useStorage must be used within a StorageProvider");
  }
  return context;
}
