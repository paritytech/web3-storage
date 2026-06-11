import { useState, useEffect, useCallback, useRef } from "react";
import {
  Archive,
  Plus,
  File,
  Folder,
  RefreshCw,
  Trash2,
  Download,
  Upload,
  ChevronRight,
  Lock,
  LockOpen,
  Copy,
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
import { formatBytes, truncateHash } from "@/lib/utils";
import { isSameAddress } from "@web3-storage/sdk";
import type { BucketInfo, S3ObjectInfo } from "@/lib/storage";
import { EncryptionKey, bytesToHex } from "@/lib/encryption";
import CreationStatusCard, { type CreationStatusItem } from "./CreationStatusCard";
import ProviderPickerDialog from "./ProviderPickerDialog";

interface S3TabProps {
  onBucketSelect?: (bucketId: bigint | null) => void;
}

export type UserRole = 'Admin' | 'Writer' | 'Reader' | null;

export default function S3Tab({ onBucketSelect }: S3TabProps) {
  const {
    buckets,
    loading,
    refreshBuckets,
    createBucket,
    deleteBucket,
    putObject,
    listObjects,
    deleteObject,
    downloadS3Object,
    signerAddress,
    waitForProvider,
    requestAgreementWithProvider,
    fetchBucketMembers,
    setEncryption,
    isEncrypted,
  } = useStorage();

  const [selectedBucket, setSelectedBucket] = useState<BucketInfo | null>(null);
  const [currentPrefix, setCurrentPrefix] = useState("");
  const [objects, setObjects] = useState<S3ObjectInfo[]>([]);
  const [loadingObjects, setLoadingObjects] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Create bucket dialog
  const [showCreateBucket, setShowCreateBucket] = useState(false);
  const [newBucketName, setNewBucketName] = useState("");
  // Defaults: 10 MB capacity (10 × 2^20 bytes), 10k blocks duration
  // maxPayment must cover: price_per_byte(1e6) × capacity × duration × 1.2 buffer
  const [bucketCapacity, setBucketCapacity] = useState("10485760");
  const [bucketDuration, setBucketDuration] = useState("10000");
  const [bucketMaxPayment, setBucketMaxPayment] = useState("120000000000000000");
  // Creation status tracking
  const [creations, setCreations] = useState<CreationStatusItem[]>([]);
  const [pickerTarget, setPickerTarget] = useState<CreationStatusItem | null>(null);

  // Role-based access
  const [userRole, setUserRole] = useState<UserRole>(null);

  // Upload dialog
  const [showUpload, setShowUpload] = useState(false);
  const [uploadKey, setUploadKey] = useState("");
  const [uploadFile, setUploadFile] = useState<globalThis.File | null>(null);

  // Encryption
  const [showEncryption, setShowEncryption] = useState(false);
  const [encryptionKeyHex, setEncryptionKeyHex] = useState("");

  // Refresh buckets on mount
  useEffect(() => {
    if (signerAddress) refreshBuckets();
  }, [signerAddress, refreshBuckets]);

  // Auto-select first bucket
  useEffect(() => {
    if (buckets.length > 0 && !selectedBucket) {
      setSelectedBucket(buckets[0]);
      setShowCreateBucket(false);
      onBucketSelect?.(buckets[0].layer0BucketId);
    }
  }, [buckets, selectedBucket, onBucketSelect]);

  // Determine user's role for the selected bucket
  useEffect(() => {
    if (!selectedBucket || !signerAddress) {
      setUserRole(null);
      return;
    }
    // The bucket owner is always Admin (the on-chain creator is added as Admin
    // in the pallet, but the member query can fail or return a format that
    // doesn't match the signer address string).
    if (isSameAddress(selectedBucket.owner, signerAddress)) {
      setUserRole('Admin');
      return;
    }
    fetchBucketMembers(selectedBucket.layer0BucketId)
      .then((members) => {
        const me = members.find((m) => isSameAddress(m.account, signerAddress));
        setUserRole(me?.role ?? null);
      })
      .catch(() => setUserRole(null));
  }, [selectedBucket, signerAddress, fetchBucketMembers]);

  const canWrite = userRole === 'Admin' || userRole === 'Writer';
  const canAdmin = userRole === 'Admin';

  // Fetch objects when bucket/prefix changes
  const refreshObjects = useCallback(async () => {
    if (!selectedBucket) return;
    setLoadingObjects(true);
    try {
      const objs = await listObjects(selectedBucket.layer0BucketId, currentPrefix || undefined);
      setObjects(objs);
    } catch {
      setObjects([]);
    } finally {
      setLoadingObjects(false);
    }
  }, [selectedBucket, currentPrefix, listObjects]);

  useEffect(() => {
    refreshObjects();
  }, [refreshObjects]);

  // Creation status helpers
  const updateCreation = useCallback((id: string, updates: Partial<CreationStatusItem>) => {
    setCreations(prev => prev.map(c => c.id === id ? { ...c, ...updates } : c));
  }, []);

  const dismissCreation = useCallback((id: string) => {
    setCreations(prev => prev.filter(c => c.id !== id));
  }, []);

  // Derive folder-like prefixes from flat keys
  const deriveFolders = (): string[] => {
    const folders = new Set<string>();
    for (const obj of objects) {
      const remainder = obj.key.slice(currentPrefix.length);
      const slash = remainder.indexOf("/");
      if (slash > 0) folders.add(currentPrefix + remainder.slice(0, slash + 1));
    }
    return Array.from(folders).sort();
  };

  // Files at current prefix level (no deeper slashes)
  const currentFiles = objects.filter((obj) => {
    const remainder = obj.key.slice(currentPrefix.length);
    return remainder.length > 0 && !remainder.includes("/");
  });

  const folders = deriveFolders();

  const breadcrumbSegments = () => {
    if (!currentPrefix) return [{ name: "/", prefix: "" }];
    const parts = currentPrefix.split("/").filter(Boolean);
    const segments = [{ name: "/", prefix: "" }];
    let acc = "";
    for (const part of parts) {
      acc += part + "/";
      segments.push({ name: part, prefix: acc });
    }
    return segments;
  };

  const validateBucketName = (name: string): boolean => {
    if (name.length < 3 || name.length > 63) return false;
    if (!/^[a-z0-9]/.test(name)) return false;
    if (!/[a-z0-9]$/.test(name)) return false;
    if (!/^[a-z0-9.-]+$/.test(name)) return false;
    return true;
  };

  const waitAndTrack = async (creationId: string, layer0BucketId: bigint): Promise<boolean> => {
    updateCreation(creationId, { stage: "waiting", elapsedMs: 0 });
    try {
      await waitForProvider(layer0BucketId, (status, elapsedMs) => {
        updateCreation(creationId, { statusMessage: status, elapsedMs });
      });
      updateCreation(creationId, { stage: "ready" });
      return true;
    } catch (err) {
      updateCreation(creationId, {
        stage: "failed",
        error: err instanceof Error ? err.message : "Provider did not accept. Try choosing a provider manually.",
      });
      return false;
    }
  };

  const [createError, setCreateError] = useState<string | null>(null);

  const handleCreateBucket = async () => {
    if (!newBucketName.trim() || !validateBucketName(newBucketName)) {
      toast({ title: "Error", description: "Invalid bucket name (3-63 chars, lowercase, S3 rules)", variant: "destructive" });
      return;
    }

    const creationId = crypto.randomUUID();
    const name = newBucketName;

    setCreations(prev => [...prev, {
      id: creationId,
      name,
      type: "bucket",
      stage: "submitting",
      elapsedMs: 0,
      createdAt: Date.now(),
    }]);

    // Close the form so the user sees the status card
    setShowCreateBucket(false);
    setNewBucketName("");
    setCreateError(null);

    try {
      const bucket = await createBucket(name, {
        capacity: BigInt(bucketCapacity),
        duration: parseInt(bucketDuration, 10),
        maxPayment: BigInt(bucketMaxPayment),
      });

      updateCreation(creationId, { stage: "created", bucketId: bucket.layer0BucketId });

      // Poll for provider acceptance in the background
      waitAndTrack(creationId, bucket.layer0BucketId).then((ready) => {
        if (!ready) return;
        // Provider accepted — select the bucket and refresh the list
        refreshBuckets();
        setSelectedBucket(bucket);
        onBucketSelect?.(bucket.layer0BucketId);
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Failed to create bucket";
      let errorMsg = msg;
      if (msg.includes("NoProvidersAvailable")) {
        errorMsg = "No storage providers available. Make sure a provider is running and accepting agreements.";
      }
      updateCreation(creationId, { stage: "failed", error: errorMsg });
    }
  };

  const handleProviderSelect = async (providerAccount: string) => {
    const bucketId = pickerTarget?.bucketId;
    if (!pickerTarget || !bucketId) return;
    const itemId = pickerTarget.id;
    setPickerTarget(null);

    updateCreation(itemId, { stage: "submitting", error: undefined });
    try {
      await requestAgreementWithProvider(
        bucketId,
        providerAccount,
        BigInt(bucketCapacity),
        parseInt(bucketDuration, 10),
        BigInt(bucketMaxPayment),
      );
      updateCreation(itemId, { stage: "created" });
      waitAndTrack(itemId, bucketId);
    } catch (err) {
      updateCreation(itemId, {
        stage: "failed",
        error: err instanceof Error ? err.message : "Agreement request failed",
      });
    }
  };

  const handleGenerateKey = async () => {
    const { rawKey } = await EncryptionKey.generate();
    const hex = bytesToHex(rawKey);
    setEncryptionKeyHex(hex);
  };

  const handleEnableEncryption = async () => {
    if (!encryptionKeyHex || encryptionKeyHex.length !== 64) {
      toast({ title: "Error", description: "Key must be 64 hex characters (32 bytes)", variant: "destructive" });
      return;
    }
    try {
      await setEncryption(encryptionKeyHex);
      toast({ title: "Encryption enabled", description: "Uploads will be encrypted client-side" });
      setShowEncryption(false);
    } catch (err) {
      toast({ title: "Error", description: err instanceof Error ? err.message : "Invalid key", variant: "destructive" });
    }
  };

  const handleDisableEncryption = async () => {
    await setEncryption(null);
    setEncryptionKeyHex("");
    toast({ title: "Encryption disabled", description: "Uploads will be sent in plaintext" });
  };

  const [deleting, setDeleting] = useState(false);

  const [confirmDelete, setConfirmDelete] = useState(false);

  const handleDeleteBucket = async () => {
    if (!selectedBucket || deleting) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    setConfirmDelete(false);
    setDeleting(true);
    try {
      await deleteBucket(selectedBucket.name);
      setSelectedBucket(null);
      setObjects([]);
      onBucketSelect?.(null);
      toast({ title: "Success", description: "Bucket deleted" });
    } catch (err) {
      toast({ title: "Error", description: err instanceof Error ? err.message : "Failed", variant: "destructive" });
    } finally {
      setDeleting(false);
    }
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

  const handleUploadObject = async () => {
    if (!selectedBucket || !uploadFile || !uploadKey.trim()) return;
    try {
      const data = await readFileAsUint8Array(uploadFile);
      await putObject(selectedBucket.name, uploadKey, data, selectedBucket.layer0BucketId, {
        contentType: uploadFile.type || "application/octet-stream",
      });
      setShowUpload(false);
      setUploadKey("");
      setUploadFile(null);
      toast({ title: "Uploaded", description: uploadKey });
      refreshObjects();
    } catch (err) {
      toast({ title: "Upload failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    }
  };

  const handleDownloadObject = async (obj: S3ObjectInfo) => {
    if (!selectedBucket) return;
    try {
      const blob = await downloadS3Object(selectedBucket.layer0BucketId, obj.key);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = obj.key.split("/").pop() || "download";
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      toast({ title: "Downloaded", description: obj.key });
    } catch (err) {
      toast({ title: "Download failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    }
  };

  const handleDeleteObject = async (obj: S3ObjectInfo) => {
    if (!selectedBucket) return;
    try {
      await deleteObject(selectedBucket.layer0BucketId, obj.key);
      toast({ title: "Deleted", description: obj.key });
      refreshObjects();
    } catch (err) {
      toast({ title: "Delete failed", description: err instanceof Error ? err.message : "Error", variant: "destructive" });
    }
  };

  // No buckets — show create form directly instead of an empty state + extra click
  useEffect(() => {
    if (buckets.length === 0 && !showCreateBucket && creations.length === 0) {
      setShowCreateBucket(true);
    }
  }, [buckets.length, showCreateBucket, creations.length]);

  return (
    <div className="space-y-4">
      {/* Bucket bar */}
      <div className="flex items-center gap-3">
        <select
          data-testid="s3-bucket-selector"
          className="flex-1 max-w-xs rounded-md border border-input bg-background px-3 py-2 text-sm"
          value={selectedBucket?.s3BucketId.toString() || ""}
          onChange={(e) => {
            const bucket = buckets.find((b) => b.s3BucketId.toString() === e.target.value);
            setSelectedBucket(bucket || null);
            setCurrentPrefix("");
            setConfirmDelete(false);
            setShowCreateBucket(false);
            onBucketSelect?.(bucket?.layer0BucketId ?? null);
          }}
        >
          {buckets.length === 0 && <option value="">No buckets</option>}
          {buckets.map((bucket) => (
            <option key={bucket.s3BucketId.toString()} value={bucket.s3BucketId.toString()}>
              {bucket.name}
            </option>
          ))}
        </select>
        <Button data-testid="s3-new-bucket" variant="outline" size="sm" onClick={() => setShowCreateBucket(true)}>
          <Plus className="mr-2 h-4 w-4" />
          New Bucket
        </Button>
        {selectedBucket && canAdmin && (
          confirmDelete ? (
            <div className="flex items-center gap-1">
              <Button data-testid="s3-delete-confirm" variant="destructive" size="sm" onClick={handleDeleteBucket} disabled={deleting}>
                {deleting ? <RefreshCw className="mr-1 h-3 w-3 animate-spin" /> : null}
                Delete
              </Button>
              <Button data-testid="s3-delete-cancel" variant="ghost" size="sm" onClick={() => setConfirmDelete(false)}>
                Cancel
              </Button>
            </div>
          ) : (
            <Button data-testid="s3-delete-bucket" variant="ghost" size="sm" onClick={handleDeleteBucket} className="text-muted-foreground hover:text-destructive">
              <Trash2 className="h-4 w-4" />
            </Button>
          )
        )}
      </div>

      {/* Create Bucket Dialog */}
      {showCreateBucket && (
        <Card data-testid="s3-create-bucket-form">
          <CardHeader>
            <CardTitle>Create New S3 Bucket</CardTitle>
            <CardDescription>Names must be 3-63 chars, lowercase, S3 naming rules</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <Input
              data-testid="s3-bucket-name-input"
              placeholder="my-bucket-name"
              value={newBucketName}
              onChange={(e) => setNewBucketName(e.target.value.toLowerCase())}
            />
            <div className="grid gap-3 md:grid-cols-3">
              <div className="space-y-1">
                <label className="text-xs font-medium">Capacity (bytes)</label>
                <Input data-testid="s3-bucket-capacity-input" type="number" value={bucketCapacity} onChange={(e) => setBucketCapacity(e.target.value)} />
                <p className="text-xs text-muted-foreground">{formatBytes(parseInt(bucketCapacity, 10) || 0)}</p>
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium">Duration (blocks)</label>
                <Input data-testid="s3-bucket-duration-input" type="number" value={bucketDuration} onChange={(e) => setBucketDuration(e.target.value)} />
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium">Max Payment</label>
                <Input data-testid="s3-bucket-maxpayment-input" type="number" value={bucketMaxPayment} onChange={(e) => setBucketMaxPayment(e.target.value)} />
              </div>
            </div>
            {createError && (
              <p data-testid="s3-create-error" className="text-sm text-destructive">{createError}</p>
            )}
            <div className="flex gap-2">
              <Button data-testid="s3-create-submit" onClick={handleCreateBucket} disabled={loading}>
                Create
              </Button>
              <Button data-testid="s3-create-cancel" variant="ghost" onClick={() => { setShowCreateBucket(false); setCreateError(null); }}>Cancel</Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Creation status cards */}
      {creations.map(item => (
        <CreationStatusCard
          key={item.id}
          item={item}
          onDismiss={dismissCreation}
          onChooseProvider={(item) => setPickerTarget(item)}
        />
      ))}

      {/* Provider picker dialog */}
      {pickerTarget && (
        <ProviderPickerDialog
          open={true}
          onClose={() => setPickerTarget(null)}
          onSelect={handleProviderSelect}
          requiredCapacity={BigInt(bucketCapacity)}
          requiredDuration={parseInt(bucketDuration, 10)}
        />
      )}

      {selectedBucket && (
        <>
          {/* Prefix breadcrumbs */}
          <div className="flex items-center gap-1 text-sm">
            {breadcrumbSegments().map((seg, i, arr) => (
              <span key={seg.prefix} className="flex items-center gap-1">
                {i > 0 && <ChevronRight className="h-3 w-3 text-muted-foreground" />}
                <button
                  className={`hover:underline ${i === arr.length - 1 ? "font-medium" : "text-muted-foreground"}`}
                  onClick={() => setCurrentPrefix(seg.prefix)}
                >
                  {seg.name}
                </button>
              </span>
            ))}
          </div>

          {/* Toolbar */}
          <div className="flex items-center gap-2">
            {canWrite && (
              <Button data-testid="s3-upload-button" variant="outline" size="sm" onClick={() => { setShowUpload(true); setUploadKey(currentPrefix); }}>
                <Upload className="mr-2 h-4 w-4" />
                Upload Object
              </Button>
            )}
            <Button
              data-testid="s3-encryption-toggle"
              variant={isEncrypted ? "default" : "outline"}
              size="sm"
              onClick={() => isEncrypted ? handleDisableEncryption() : setShowEncryption(!showEncryption)}
              title={isEncrypted ? "Encryption active — click to disable" : "Enable client-side encryption"}
            >
              {isEncrypted ? <Lock className="mr-2 h-4 w-4" /> : <LockOpen className="mr-2 h-4 w-4" />}
              {isEncrypted ? "Encrypted" : "Encrypt"}
            </Button>
            <Button data-testid="s3-refresh-objects" variant="ghost" size="sm" onClick={refreshObjects} disabled={loadingObjects}>
              <RefreshCw className={`h-4 w-4 ${loadingObjects ? "animate-spin" : ""}`} />
            </Button>
            {userRole && !canAdmin && (
              <span data-testid="s3-user-role" className="text-xs text-muted-foreground ml-2">Role: {userRole}</span>
            )}
          </div>

          {/* Encryption key setup */}
          {showEncryption && !isEncrypted && (
            <Card data-testid="s3-encryption-form">
              <CardContent className="pt-4 space-y-3">
                <div className="space-y-1">
                  <label className="text-xs font-medium">Encryption Key (64 hex chars = 32 bytes)</label>
                  <div className="flex items-center gap-2">
                    <Input
                      data-testid="s3-encryption-key-input"
                      placeholder="Enter or generate a 256-bit hex key"
                      value={encryptionKeyHex}
                      onChange={(e) => setEncryptionKeyHex(e.target.value.replace(/[^0-9a-fA-F]/g, "").slice(0, 64))}
                      className="font-mono text-xs"
                    />
                    <Button data-testid="s3-encryption-generate" variant="outline" size="sm" onClick={handleGenerateKey} title="Generate random key">
                      Generate
                    </Button>
                    {encryptionKeyHex && (
                      <Button data-testid="s3-encryption-copy" variant="ghost" size="icon" className="h-8 w-8" onClick={() => {
                        navigator.clipboard.writeText(encryptionKeyHex);
                        toast({ title: "Copied", description: "Key copied to clipboard" });
                      }} title="Copy key">
                        <Copy className="h-3.5 w-3.5" />
                      </Button>
                    )}
                  </div>
                  <p className="text-xs text-muted-foreground">
                    Save this key securely — you need it to decrypt your files. Lost keys cannot be recovered.
                  </p>
                </div>
                <div className="flex gap-2">
                  <Button data-testid="s3-encryption-enable" size="sm" onClick={handleEnableEncryption} disabled={encryptionKeyHex.length !== 64}>
                    <Lock className="mr-2 h-4 w-4" />
                    Enable Encryption
                  </Button>
                  <Button data-testid="s3-encryption-cancel" variant="ghost" size="sm" onClick={() => setShowEncryption(false)}>Cancel</Button>
                </div>
              </CardContent>
            </Card>
          )}

          {/* Upload inline */}
          {showUpload && (
            <Card data-testid="s3-upload-form">
              <CardContent className="pt-4 space-y-3">
                <Input
                  data-testid="s3-upload-key-input"
                  placeholder="Object key (e.g. uploads/photo.jpg)"
                  value={uploadKey}
                  onChange={(e) => setUploadKey(e.target.value)}
                />
                <div className="flex items-center gap-2">
                  <Button data-testid="s3-upload-choose-file" variant="outline" size="sm" onClick={() => fileInputRef.current?.click()}>
                    {uploadFile ? uploadFile.name : "Choose File"}
                  </Button>
                  <input data-testid="s3-upload-file-input" ref={fileInputRef} type="file" className="hidden" onChange={(e) => {
                    const file = e.target.files?.[0];
                    if (file) {
                      setUploadFile(file);
                      // Auto-fill key from filename if key is empty or just the prefix
                      if (!uploadKey || uploadKey === currentPrefix) {
                        setUploadKey(currentPrefix + file.name);
                      }
                    }
                  }} />
                  <Button data-testid="s3-upload-submit" size="sm" onClick={handleUploadObject} disabled={!uploadFile || !uploadKey}>Upload</Button>
                  <Button data-testid="s3-upload-cancel" variant="ghost" size="sm" onClick={() => { setShowUpload(false); setUploadFile(null); }}>Cancel</Button>
                </div>
              </CardContent>
            </Card>
          )}

          {/* Object list */}
          <div className="rounded-lg border">
            {loadingObjects ? (
              <div className="p-8 text-center text-muted-foreground">
                <RefreshCw className="mx-auto h-8 w-8 mb-2 animate-spin opacity-50" />
                <p>Loading objects...</p>
              </div>
            ) : folders.length === 0 && currentFiles.length === 0 ? (
              <div className="p-8 text-center text-muted-foreground">
                <Archive className="mx-auto h-8 w-8 mb-2 opacity-50" />
                <p>This bucket is empty</p>
                <p className="text-sm">Upload objects to get started</p>
              </div>
            ) : (
              <table data-testid="s3-objects-table" className="w-full">
                <thead>
                  <tr className="border-b bg-muted/50">
                    <th className="px-4 py-2 text-left text-sm font-medium">Key</th>
                    <th className="px-4 py-2 text-left text-sm font-medium w-24">Size</th>
                    <th className="px-4 py-2 text-left text-sm font-medium w-44">Last Modified</th>
                    <th className="px-4 py-2 text-left text-sm font-medium w-28">ETag</th>
                    <th className="px-4 py-2 text-right text-sm font-medium w-24">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {/* Folder rows */}
                  {folders.map((prefix) => {
                    const folderName = prefix.slice(currentPrefix.length).replace(/\/$/, "");
                    return (
                      <tr key={prefix} data-testid={`s3-folder-row-${folderName}`} className="border-b hover:bg-muted/30">
                        <td className="px-4 py-2" colSpan={4}>
                          <button
                            className="flex items-center gap-2 hover:underline"
                            onClick={() => setCurrentPrefix(prefix)}
                          >
                            <Folder className="h-4 w-4 text-yellow-500" />
                            <span className="text-sm font-medium">{folderName}/</span>
                          </button>
                        </td>
                        <td />
                      </tr>
                    );
                  })}
                  {/* Object rows */}
                  {currentFiles.map((obj) => (
                    <tr key={obj.key} data-testid={`s3-object-row-${obj.key}`} className="border-b hover:bg-muted/30">
                      <td className="px-4 py-2">
                        <span className="flex items-center gap-2 text-sm">
                          <File className="h-4 w-4 text-muted-foreground" />
                          {obj.key.slice(currentPrefix.length)}
                        </span>
                      </td>
                      <td className="px-4 py-2 text-sm text-muted-foreground">{formatBytes(obj.size)}</td>
                      <td className="px-4 py-2 text-sm text-muted-foreground">
                        {new Date(obj.lastModified).toLocaleString()}
                      </td>
                      <td className="px-4 py-2 font-mono text-xs text-muted-foreground">
                        {truncateHash(obj.etag, 6, 4)}
                      </td>
                      <td className="px-4 py-2 text-right">
                        <div className="flex items-center justify-end gap-1">
                          <Button data-testid={`s3-download-${obj.key}`} variant="ghost" size="icon" className="h-7 w-7" onClick={() => handleDownloadObject(obj)}>
                            <Download className="h-3.5 w-3.5" />
                          </Button>
                          {canWrite && (
                            <Button data-testid={`s3-delete-object-${obj.key}`} variant="ghost" size="icon" className="h-7 w-7 text-muted-foreground hover:text-destructive" onClick={() => handleDeleteObject(obj)}>
                              <Trash2 className="h-3.5 w-3.5" />
                            </Button>
                          )}
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
