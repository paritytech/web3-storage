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
  type BucketInfo,
  type BucketMember,
  type ProviderEndpointInfo,
  type AvailableProvider,
  type CreateBucketOptions,
  type UploadResult,
  type PutObjectOptions,
  type S3ObjectInfo,
  type CheckpointStatus,
} from "@/lib/storage";
import { EncryptionKey, hexToBytes } from "@/lib/encryption";
import { useChain } from "./useChain";

interface StorageState {
  client: StorageClient | null;
  signerAddress: string | null;
  signerName: string | null;
  buckets: BucketInfo[];
  loading: boolean;
  error: string | null;
  balance: { free: bigint; reserved: bigint } | null;
  providerCapacity: { max: bigint; used: bigint } | null;

  // Account
  setSigner: (seed: string) => Promise<void>;

  // Buckets (S3)
  createBucket: (name: string, options: CreateBucketOptions) => Promise<BucketInfo>;
  refreshBuckets: () => Promise<void>;
  deleteBucket: (name: string) => Promise<void>;
  putObject: (bucketName: string, key: string, data: Uint8Array, bucketId: bigint, options?: PutObjectOptions) => Promise<UploadResult>;
  downloadS3Object: (bucketId: bigint, key: string) => Promise<Blob>;
  listObjects: (bucketId: bigint, prefix?: string) => Promise<S3ObjectInfo[]>;
  deleteObject: (bucketId: bigint, key: string) => Promise<void>;

  // Provider
  checkProviderHealth: (bucketId: bigint) => Promise<boolean>;
  waitForProvider: (bucketId: bigint, onProgress?: (status: string, elapsedMs: number, attempt: number) => void) => Promise<string>;
  listAvailableProviders: () => Promise<AvailableProvider[]>;
  requestAgreementWithProvider: (bucketId: bigint, providerAccount: string, maxBytes: bigint, duration: number, maxPayment: bigint) => Promise<void>;

  // Bucket Members & Permissions
  fetchBucketMembers: (bucketId: bigint) => Promise<BucketMember[]>;
  addBucketMember: (bucketId: bigint, account: string, role: 'Admin' | 'Writer' | 'Reader') => Promise<void>;
  removeBucketMember: (bucketId: bigint, account: string) => Promise<void>;
  fetchBucketProviders: (bucketId: bigint) => Promise<ProviderEndpointInfo[]>;

  // Shared Drives
  listAccessibleBucketIds: () => Promise<bigint[]>;

  // Checkpoints
  fetchCheckpointStatus: (bucketId: bigint) => Promise<CheckpointStatus | null>;
  submitCheckpoint: (bucketId: bigint) => Promise<void>;
  configureCheckpointWindow: (bucketId: bigint, interval: number, gracePeriod: number, enabled: boolean) => Promise<void>;
  fundCheckpointPool: (bucketId: bigint, amount: bigint) => Promise<void>;
  claimCheckpointRewards: (bucketId: bigint) => Promise<void>;

  // Encryption
  setEncryption: (keyHex: string | null) => Promise<void>;
  isEncrypted: boolean;
}

const StorageContext = createContext<StorageState | null>(null);

export function StorageProvider({ children }: { children: ReactNode }) {
  const { connected, chainEndpoint, blockNumber } = useChain();
  const [client, setClient] = useState<StorageClient | null>(null);
  const [signerAddress, setSignerAddress] = useState<string | null>(null);
  const [signerName, setSignerName] = useState<string | null>(null);
  const [buckets, setBuckets] = useState<BucketInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [balance, setBalance] = useState<{ free: bigint; reserved: bigint } | null>(null);
  const [providerCapacity, setProviderCapacity] = useState<{ max: bigint; used: bigint } | null>(null);
  const [isEncrypted, setIsEncrypted] = useState(false);

  // Initialize client when chain is connected; auto-set Alice on local dev chains
  useEffect(() => {
    if (connected && chainEndpoint) {
      console.log("[useStorage] Initializing client:", { chainEndpoint });
      const newClient = new StorageClient(chainEndpoint);
      newClient.connect().then(async () => {
        console.log("[useStorage] Client connected successfully");
        setClient(newClient);

        // Auto-set Alice on local dev chains so the UI is usable immediately
        if (chainEndpoint.includes("127.0.0.1") || chainEndpoint.includes("localhost")) {
          try {
            const address = await newClient.setSigner("//Alice");
            setSignerAddress(address);
            setSignerName("Alice");
            console.log("[useStorage] Auto-set signer to Alice:", address);
          } catch (err) {
            console.warn("[useStorage] Auto-signer failed (not critical):", err);
          }
        }
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

  // Fetch balance and buckets when signer changes, then poll every 15s
  // so external changes (CLI tools, other clients) are reflected.
  useEffect(() => {
    if (!client || !signerAddress) {
      setBalance(null);
      setBuckets([]);
      return;
    }

    const refresh = () => {
      client.getBalance(signerAddress).then(setBalance).catch(() => setBalance(null));
      client.listBuckets().then(setBuckets).catch(() => setBuckets([]));
    };

    refresh();
    const interval = setInterval(refresh, 15_000);
    return () => clearInterval(interval);
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

  const downloadS3Object = useCallback(async (bucketId: bigint, key: string): Promise<Blob> => {
    if (!client) throw new Error("Client not connected");
    return client.downloadS3Object(bucketId, key);
  }, [client]);

  const listObjects = useCallback(async (bucketId: bigint, prefix?: string): Promise<S3ObjectInfo[]> => {
    if (!client) throw new Error("Client not connected");
    return client.listObjects(bucketId, prefix);
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
    onProgress?: (status: string, elapsedMs: number, attempt: number) => void,
  ): Promise<string> => {
    if (!client) throw new Error("Client not connected");
    return client.waitForProvider(bucketId, onProgress);
  }, [client]);

  const listAvailableProviders = useCallback(async (): Promise<AvailableProvider[]> => {
    if (!client) throw new Error("Client not connected");
    return client.listAvailableProviders();
  }, [client]);

  const requestAgreementWithProvider = useCallback(async (
    bucketId: bigint,
    providerAccount: string,
    maxBytes: bigint,
    duration: number,
    maxPayment: bigint,
  ): Promise<void> => {
    if (!client) throw new Error("Client not connected");
    setError(null);
    try {
      await client.requestAgreementWithProvider(bucketId, providerAccount, maxBytes, duration, maxPayment);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to request agreement";
      setError(msg);
      throw err;
    }
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

  // --- Shared Drives ---

  const listAccessibleBucketIds = useCallback(async (): Promise<bigint[]> => {
    if (!client) return [];
    return client.listAccessibleBucketIds();
  }, [client]);

  // --- Encryption ---

  const setEncryption = useCallback(async (keyHex: string | null) => {
    if (!client) throw new Error("Client not connected");
    if (keyHex) {
      const rawKey = hexToBytes(keyHex);
      const encKey = await EncryptionKey.fromBytes(rawKey);
      client.setEncryptionKey(encKey);
      setIsEncrypted(true);
    } else {
      client.clearEncryptionKey();
      setIsEncrypted(false);
    }
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
        buckets,
        loading,
        error,
        balance,
        providerCapacity,
        setSigner,
        createBucket,
        refreshBuckets,
        deleteBucket,
        putObject,
        downloadS3Object,
        listObjects,
        deleteObject,
        checkProviderHealth,
        waitForProvider,
        listAvailableProviders,
        requestAgreementWithProvider,
        fetchBucketMembers,
        addBucketMember,
        removeBucketMember,
        fetchBucketProviders,
        listAccessibleBucketIds,
        fetchCheckpointStatus,
        submitCheckpoint,
        configureCheckpointWindow,
        fundCheckpointPool,
        claimCheckpointRewards,
        setEncryption,
        isEncrypted,
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
