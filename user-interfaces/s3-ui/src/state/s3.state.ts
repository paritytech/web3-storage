// SPDX-License-Identifier: GPL-3.0-only

/**
 * S3 State — bucket/object orchestration over an S3Client.
 *
 * Owns the S3Client instance, syncing it with chain.state.ts (api) and
 * wallet.state.ts (signer). Holds buckets/objects/selection state. Subscribes
 * to S3Registry events for real-time updates.
 */

import { BehaviorSubject, combineLatest, distinctUntilChanged, Subscription } from "rxjs";
import { bind } from "@react-rxjs/core";
import {
  S3Client,
  type AvailableProvider,
  type BucketInfo,
  type MatchingProviders,
  type QueryMatchingProvidersParams,
  type S3ObjectInfo,
  type SignedTerms,
} from "@/lib/s3-client";
import { EncryptionKey, hexToBytes } from "@/lib/encryption";
import { api$$, getApi } from "@/state/chain.state";
import { signer$$, keypair$$, signerAddress$$, getSignerAddress, refreshBalance } from "@/state/wallet.state";

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

export type CreationStage = "submitting" | "ready" | "failed";

export interface CreationStatus {
  id: string;
  name: string;
  stage: CreationStage;
  elapsedMs: number;
  error?: string;
  s3BucketId?: bigint;
}

export interface CreateBucketInput {
  name: string;
  provider: AvailableProvider;
  url: string;
  signed: SignedTerms;
}

export type ViewMode = "list" | "grid";

// ─────────────────────────────────────────────────────────────────────────────
// localStorage hydration
// ─────────────────────────────────────────────────────────────────────────────

const STORAGE_VIEW_MODE = "s3-ui-view-mode";
const STORAGE_SELECTED_BUCKET = "s3-ui-selected-bucket";
const STORAGE_CURRENT_PREFIX = "s3-ui-current-prefix";

function readViewMode(): ViewMode {
  const v = localStorage.getItem(STORAGE_VIEW_MODE);
  return v === "grid" ? "grid" : "list";
}

function readSelectedBucketId(): bigint | null {
  const v = localStorage.getItem(STORAGE_SELECTED_BUCKET);
  if (!v) return null;
  try {
    return BigInt(v);
  } catch {
    return null;
  }
}

function readCurrentPrefix(): string {
  return localStorage.getItem(STORAGE_CURRENT_PREFIX) || "";
}

// ─────────────────────────────────────────────────────────────────────────────
// Client lifecycle (one S3Client per session, kept in sync with api/signer)
// ─────────────────────────────────────────────────────────────────────────────

const client = new S3Client();

api$$.subscribe((api) => {
  client.setApi(api);
});

combineLatest([signer$$, signerAddress$$]).subscribe(([signer, address]) => {
  client.setSigner(signer, address);
});

keypair$$.subscribe((keypair) => {
  client.setKeypair(keypair);
});

export function getS3Client(): S3Client {
  return client;
}

// ─────────────────────────────────────────────────────────────────────────────
// State subjects
// ─────────────────────────────────────────────────────────────────────────────

const buckets$ = new BehaviorSubject<BucketInfo[]>([]);
const selectedBucket$ = new BehaviorSubject<BucketInfo | null>(null);
const currentPrefix$ = new BehaviorSubject<string>(readCurrentPrefix());
const objects$ = new BehaviorSubject<S3ObjectInfo[]>([]);
const loading$ = new BehaviorSubject<boolean>(false);
const uploading$ = new BehaviorSubject<{ active: boolean; progress: number }>({
  active: false,
  progress: 0,
});
const error$ = new BehaviorSubject<string | null>(null);
const viewMode$ = new BehaviorSubject<ViewMode>(readViewMode());
const creations$ = new BehaviorSubject<CreationStatus[]>([]);
const encryptionKey$ = new BehaviorSubject<EncryptionKey | null>(null);

let uploadAbortController: AbortController | null = null;
let pendingSelectedBucketId: bigint | null = readSelectedBucketId();

// Persist viewMode + currentPrefix + selected bucket id
viewMode$.subscribe((mode) => localStorage.setItem(STORAGE_VIEW_MODE, mode));
currentPrefix$.subscribe((prefix) => localStorage.setItem(STORAGE_CURRENT_PREFIX, prefix));
selectedBucket$.subscribe((b) => {
  if (b) localStorage.setItem(STORAGE_SELECTED_BUCKET, b.s3BucketId.toString());
  else localStorage.removeItem(STORAGE_SELECTED_BUCKET);
});

// ─────────────────────────────────────────────────────────────────────────────
// Hooks
// ─────────────────────────────────────────────────────────────────────────────

export const [useBuckets] = bind(buckets$, []);
export const [useSelectedBucket] = bind(selectedBucket$, null);
export const [useCurrentPrefix] = bind(currentPrefix$, "");
export const [useObjects] = bind(objects$, []);
export const [useS3Loading] = bind(loading$, false);
export const [useUploading] = bind(uploading$, { active: false, progress: 0 });
export const [useS3Error] = bind(error$, null);
export const [useViewMode] = bind(viewMode$, "list");
export const [useCreations] = bind(creations$, []);
export const [useEncryptionKey] = bind(encryptionKey$, null);

// ─────────────────────────────────────────────────────────────────────────────
// Bucket CRUD
// ─────────────────────────────────────────────────────────────────────────────

export async function refreshBuckets(): Promise<void> {
  if (!client.hasApi() || !client.hasSigner()) return;
  loading$.next(true);
  try {
    const list = await client.listBuckets();
    buckets$.next(list);

    const sel = selectedBucket$.getValue();
    if (sel) {
      const updated = list.find((b) => b.s3BucketId === sel.s3BucketId) ?? null;
      if (updated && updated.name !== sel.name) {
        selectedBucket$.next(updated);
      } else if (!updated) {
        selectedBucket$.next(null);
        objects$.next([]);
      }
    } else if (pendingSelectedBucketId !== null) {
      const persisted = list.find((b) => b.s3BucketId === pendingSelectedBucketId);
      if (persisted) {
        selectedBucket$.next(persisted);
      }
      pendingSelectedBucketId = null;
    }

    error$.next(null);
  } catch (err) {
    error$.next(err instanceof Error ? err.message : "Failed to load buckets");
  } finally {
    loading$.next(false);
  }
}

export async function selectBucket(bucket: BucketInfo | null): Promise<void> {
  selectedBucket$.next(bucket);
  if (!bucket) {
    objects$.next([]);
    currentPrefix$.next("");
    return;
  }
  currentPrefix$.next("");
}

export function navigateToPrefix(prefix: string): void {
  currentPrefix$.next(prefix);
}

export function navigateUp(): void {
  const prefix = currentPrefix$.getValue();
  if (!prefix) return;
  const parts = prefix.split("/").filter(Boolean);
  parts.pop();
  currentPrefix$.next(parts.length === 0 ? "" : parts.join("/") + "/");
}

export async function refreshObjects(): Promise<void> {
  const bucket = selectedBucket$.getValue();
  if (!bucket || !client.hasApi()) return;
  loading$.next(true);
  error$.next(null);
  try {
    const list = await client.listObjects(bucket.layer0BucketId, currentPrefix$.getValue() || undefined);
    objects$.next(list);
  } catch (err) {
    error$.next(err instanceof Error ? err.message : "Failed to list objects");
    objects$.next([]);
  } finally {
    loading$.next(false);
  }
}

export function setViewMode(mode: ViewMode): void {
  viewMode$.next(mode);
}

// ─────────────────────────────────────────────────────────────────────────────
// Object operations
// ─────────────────────────────────────────────────────────────────────────────

function readFileAsUint8Array(file: File): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      if (reader.result instanceof ArrayBuffer) resolve(new Uint8Array(reader.result));
      else reject(new Error("Failed to read file"));
    };
    reader.onerror = () => reject(reader.error ?? new Error("FileReader failed"));
    reader.readAsArrayBuffer(file);
  });
}

export async function uploadFiles(files: File[]): Promise<void> {
  const bucket = selectedBucket$.getValue();
  if (!bucket || !client.hasApi()) return;
  if (files.length === 0) return;

  uploadAbortController = new AbortController();
  const signal = uploadAbortController.signal;
  uploading$.next({ active: true, progress: 0 });

  const uploaded: string[] = [];
  let aborted = false;

  try {
    const prefix = currentPrefix$.getValue();
    for (let i = 0; i < files.length; i++) {
      if (signal.aborted) {
        aborted = true;
        break;
      }
      const file = files[i]!;
      let data = await readFileAsUint8Array(file);

      // Encrypt if encryption key is set
      const key = encryptionKey$.getValue();
      if (key) {
        data = await key.encrypt(data);
      }

      const objectKey = prefix ? `${prefix}${file.name}` : file.name;
      await client.putObject(bucket.layer0BucketId, objectKey, data, { signal });
      uploaded.push(file.name);
      uploading$.next({ active: true, progress: ((i + 1) / files.length) * 100 });
    }
  } finally {
    uploading$.next({ active: false, progress: 0 });
    uploadAbortController = null;
    if (uploaded.length > 0) {
      await refreshObjects();
    }
  }

  if (aborted && uploaded.length < files.length) {
    throw new DOMException("Upload aborted", "AbortError");
  }
}

export async function uploadObject(key: string, file: File): Promise<void> {
  const bucket = selectedBucket$.getValue();
  if (!bucket || !client.hasApi()) return;

  uploading$.next({ active: true, progress: 0 });
  try {
    let data = await readFileAsUint8Array(file);

    const eKey = encryptionKey$.getValue();
    if (eKey) {
      data = await eKey.encrypt(data);
    }

    await client.putObject(bucket.layer0BucketId, key, data);
    uploading$.next({ active: true, progress: 100 });
    await refreshObjects();
  } finally {
    uploading$.next({ active: false, progress: 0 });
  }
}

export function abortUpload(): void {
  uploadAbortController?.abort();
}

export async function downloadObject(key: string): Promise<Uint8Array> {
  const bucket = selectedBucket$.getValue();
  if (!bucket) throw new Error("No bucket selected");
  let data = await client.getObject(bucket.layer0BucketId, key);

  // Decrypt if encryption key is set and data looks encrypted (version byte 0x02)
  const eKey = encryptionKey$.getValue();
  if (eKey && data.length > 0 && data[0] === 0x02) {
    data = await eKey.decrypt(data);
  }

  return data;
}

export async function deleteObject(key: string): Promise<void> {
  const bucket = selectedBucket$.getValue();
  if (!bucket) return;
  await client.deleteObject(bucket.layer0BucketId, key);
  await refreshObjects();
}

// ─────────────────────────────────────────────────────────────────────────────
// Encryption
// ─────────────────────────────────────────────────────────────────────────────

export async function setEncryptionKey(hexKey: string): Promise<void> {
  const rawKey = hexToBytes(hexKey);
  encryptionKey$.next(await EncryptionKey.fromBytes(rawKey));
}

export function clearEncryptionKey(): void {
  encryptionKey$.next(null);
}

// ─────────────────────────────────────────────────────────────────────────────
// Bucket lifecycle
// ─────────────────────────────────────────────────────────────────────────────

function updateCreation(id: string, updates: Partial<CreationStatus>): void {
  creations$.next(creations$.getValue().map((c) => (c.id === id ? { ...c, ...updates } : c)));
}

export function dismissCreation(id: string): void {
  creations$.next(creations$.getValue().filter((c) => c.id !== id));
}

interface RetryCtx {
  provider: AvailableProvider;
  url: string;
  signed: SignedTerms;
}
const retryCtx = new Map<string, RetryCtx>();

export function canRetryCreation(id: string): boolean {
  return retryCtx.has(id);
}

async function runChainSubmit(id: string, ctx: RetryCtx): Promise<BucketInfo | null> {
  updateCreation(id, { stage: "submitting", error: undefined });
  try {
    // Name lives on the creation record (keyed by id), not the retry context.
    const name = creations$.getValue().find((c) => c.id === id)?.name ?? "";
    const bucket = await client.createBucket(name, ctx.provider.account, ctx.url, ctx.signed);
    updateCreation(id, { stage: "ready", s3BucketId: bucket.s3BucketId });
    retryCtx.delete(id);
    await refreshBuckets();
    const refreshed = buckets$.getValue().find((b) => b.s3BucketId === bucket.s3BucketId) ?? bucket;
    await selectBucket(refreshed);
    await refreshBalance();
    return refreshed;
  } catch (err) {
    updateCreation(id, {
      stage: "failed",
      error: err instanceof Error ? err.message : "Failed to submit on chain",
    });
    return null;
  }
}

export async function createBucket(input: CreateBucketInput): Promise<BucketInfo | null> {
  if (!client.hasApi() || !client.hasSigner()) return null;

  const id = crypto.randomUUID();
  creations$.next([
    ...creations$.getValue(),
    { id, name: input.name, stage: "submitting", elapsedMs: 0 },
  ]);

  const ctx: RetryCtx = {
    provider: input.provider,
    url: input.url,
    signed: input.signed,
  };
  retryCtx.set(id, ctx);
  return runChainSubmit(id, ctx);
}

export async function retryCreation(
  id: string,
  name?: string,
): Promise<BucketInfo | null> {
  const ctx = retryCtx.get(id);
  if (!ctx) return null;
  // Let the caller override the name on retry (e.g. from the current input).
  // runChainSubmit reads the name back off the creation record.
  if (name !== undefined) updateCreation(id, { name });
  return runChainSubmit(id, ctx);
}

export async function listAvailableProviders(): Promise<AvailableProvider[]> {
  if (!client.hasApi()) return [];
  return client.listAvailableProviders();
}

const DEFAULT_PROVIDER_LIMIT = 10;
export async function queryMatchingProviders(
  query: QueryMatchingProvidersParams["query"],
  limit: QueryMatchingProvidersParams["limit"] = DEFAULT_PROVIDER_LIMIT,
): Promise<MatchingProviders[]> {
  if (!client.hasApi()) return [];
  return client.queryMatchingProviders(query, limit);
}

export async function deleteBucket(s3BucketId: bigint): Promise<void> {
  if (!client.hasApi() || !client.hasSigner()) return;
  await client.deleteBucket(s3BucketId);
  if (selectedBucket$.getValue()?.s3BucketId === s3BucketId) {
    selectedBucket$.next(null);
    objects$.next([]);
    currentPrefix$.next("");
  }
  await refreshBuckets();
  await refreshBalance();
}

// ─────────────────────────────────────────────────────────────────────────────
// Members
// ─────────────────────────────────────────────────────────────────────────────

export async function fetchMembers(bucketId: bigint) {
  if (!client.hasApi()) return [];
  return client.getBucketMembers(bucketId);
}

export async function addMember(
  bucketId: bigint,
  account: string,
  role: import("@/lib/s3-client").MemberRole,
): Promise<void> {
  await client.addMember(bucketId, account, role);
}

export async function removeMember(bucketId: bigint, account: string): Promise<void> {
  await client.removeMember(bucketId, account);
}

// ─────────────────────────────────────────────────────────────────────────────
// Real-time S3Registry event subscription
// ─────────────────────────────────────────────────────────────────────────────

let eventSub: Subscription | null = null;

function subscribeToS3Events(): void {
  eventSub?.unsubscribe();
  eventSub = null;
  const api = getApi();
  if (!api) return;

  const handle = (s3BucketId: bigint, owner: string): void => {
    const ownAddr = getSignerAddress();
    const tracked = new Set(buckets$.getValue().map((b) => b.s3BucketId));
    if (owner !== ownAddr && !tracked.has(s3BucketId)) return;
    queueMicrotask(() => refreshBuckets().catch(() => {}));
  };

  eventSub = new Subscription();
  eventSub.add(
    api.event.S3Registry.S3BucketCreated.watch().subscribe({
      next: ({ events }) => events.forEach((ev) => handle(ev.payload.s3_bucket_id, ev.payload.owner)),
      error: () => {},
    }),
  );
  eventSub.add(
    api.event.S3Registry.S3BucketDeleted.watch().subscribe({
      next: ({ events }) => events.forEach((ev) => handle(ev.payload.s3_bucket_id, "")),
      error: () => {},
    }),
  );
}

api$$.subscribe(() => {
  subscribeToS3Events();
});

// ─────────────────────────────────────────────────────────────────────────────
// Reactive: refresh buckets when api+signer become available; refresh objects
// when (selectedBucket, currentPrefix) change.
// ─────────────────────────────────────────────────────────────────────────────

combineLatest([api$$, signerAddress$$])
  .pipe(distinctUntilChanged((a, b) => a[0] === b[0] && a[1] === b[1]))
  .subscribe(([api, address]) => {
    if (api && address) {
      buckets$.next([]);
      selectedBucket$.next(null);
      objects$.next([]);
      currentPrefix$.next("");
      refreshBuckets().catch(() => {});
    } else {
      buckets$.next([]);
      selectedBucket$.next(null);
      objects$.next([]);
    }
  });

combineLatest([selectedBucket$, currentPrefix$])
  .pipe(distinctUntilChanged((a, b) => a[0]?.s3BucketId === b[0]?.s3BucketId && a[1] === b[1]))
  .subscribe(([bucket]) => {
    if (bucket && client.hasApi()) {
      refreshObjects().catch(() => {});
    }
  });

// ─────────────────────────────────────────────────────────────────────────────
// Non-reactive getters
// ─────────────────────────────────────────────────────────────────────────────

export function getBuckets(): BucketInfo[] {
  return buckets$.getValue();
}

export function getSelectedBucket(): BucketInfo | null {
  return selectedBucket$.getValue();
}

export function getCurrentPrefix(): string {
  return currentPrefix$.getValue();
}
