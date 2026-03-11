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
  type CreateDriveOptions,
  type CreateBucketOptions,
  type UploadResult,
  type PutObjectOptions,
  type S3ObjectInfo,
} from "@/lib/storage";
import { useChain } from "./useChain";

interface StorageState {
  client: StorageClient | null;
  signerAddress: string | null;
  drives: DriveInfo[];
  buckets: BucketInfo[];
  loading: boolean;
  error: string | null;

  // Account
  setSigner: (seed: string) => Promise<void>;

  // Drives (File System)
  createDrive: (options: CreateDriveOptions) => Promise<bigint>;
  refreshDrives: () => Promise<void>;
  deleteDrive: (driveId: bigint) => Promise<void>;
  uploadToDrive: (driveId: bigint, bucketId: bigint, path: string, data: Uint8Array) => Promise<UploadResult>;
  downloadFromDrive: (bucketId: bigint, cid: string) => Promise<Uint8Array>;

  // Buckets (S3)
  createBucket: (name: string, options: CreateBucketOptions) => Promise<BucketInfo>;
  refreshBuckets: () => Promise<void>;
  deleteBucket: (name: string) => Promise<void>;
  putObject: (bucketName: string, key: string, data: Uint8Array, bucketId: bigint, options?: PutObjectOptions) => Promise<UploadResult>;
  getObject: (bucketId: bigint, cid: string) => Promise<Uint8Array>;
  listObjects: (bucketId: bigint, prefix?: string) => Promise<S3ObjectInfo[]>;

  // Provider
  checkProviderHealth: () => Promise<boolean>;
}

const StorageContext = createContext<StorageState | null>(null);

export function StorageProvider({ children }: { children: ReactNode }) {
  const { connected, chainEndpoint, providerEndpoint } = useChain();
  const [client, setClient] = useState<StorageClient | null>(null);
  const [signerAddress, setSignerAddress] = useState<string | null>(null);
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [buckets, setBuckets] = useState<BucketInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Initialize client when chain is connected
  useEffect(() => {
    if (connected && chainEndpoint && providerEndpoint) {
      console.log("[useStorage] Initializing client:", { chainEndpoint, providerEndpoint });
      const newClient = new StorageClient(chainEndpoint, providerEndpoint);
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
      setDrives([]);
      setBuckets([]);
    }
  }, [connected, chainEndpoint, providerEndpoint]);

  const setSigner = useCallback(async (seed: string) => {
    if (!client) throw new Error("Client not connected");
    setLoading(true);
    setError(null);
    try {
      const address = await client.setSigner(seed);
      setSignerAddress(address);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to set signer");
      throw err;
    } finally {
      setLoading(false);
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

  // --- Provider Health ---

  const checkProviderHealth = useCallback(async (): Promise<boolean> => {
    if (!client) return false;
    return client.checkProviderHealth();
  }, [client]);

  return (
    <StorageContext.Provider
      value={{
        client,
        signerAddress,
        drives,
        buckets,
        loading,
        error,
        setSigner,
        createDrive,
        refreshDrives,
        deleteDrive,
        uploadToDrive,
        downloadFromDrive,
        createBucket,
        refreshBuckets,
        deleteBucket,
        putObject,
        getObject,
        listObjects,
        checkProviderHealth,
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
