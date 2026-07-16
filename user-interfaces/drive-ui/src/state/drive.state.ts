// SPDX-License-Identifier: GPL-3.0-only

/**
 * Drive State - file system orchestration over a DriveClient.
 *
 * Owns the DriveClient instance, syncing it with chain.state.ts (api) and
 * wallet.state.ts (signer). Holds drives/entries/selection state. Subscribes
 * to DriveRegistry events for real-time updates.
 */

import { BehaviorSubject, combineLatest, distinctUntilChanged, Subscription } from "rxjs";
import { bind } from "@react-rxjs/core";
import {
  DriveClient,
  MatchingProviders,
  QueryMatchingProvidersParams,
  type AvailableProvider,
  type DriveInfo,
  type FsEntry,
  type SignedTerms,
} from "@/lib/drive-client";
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
  bucketId?: bigint;
}

export interface CreateDriveInput {
  name?: string;
  /** The provider the user picked. */
  provider: AvailableProvider;
  /** Provider HTTP endpoint (parsed from its multiaddr). */
  url: string;
  /** Terms already negotiated with the provider (`POST /negotiate`). */
  signed: SignedTerms;
}

export type ViewMode = "list" | "grid";

// ─────────────────────────────────────────────────────────────────────────────
// localStorage hydration
// ─────────────────────────────────────────────────────────────────────────────

const STORAGE_VIEW_MODE = "drive-ui-view-mode";
const STORAGE_SELECTED_DRIVE = "drive-ui-selected-drive";
const STORAGE_CURRENT_PATH = "drive-ui-current-path";

function readViewMode(): ViewMode {
  const v = localStorage.getItem(STORAGE_VIEW_MODE);
  return v === "grid" ? "grid" : "list";
}

function readSelectedDriveId(): bigint | null {
  const v = localStorage.getItem(STORAGE_SELECTED_DRIVE);
  if (!v) return null;
  try {
    return BigInt(v);
  } catch {
    return null;
  }
}

function readCurrentPath(): string {
  return localStorage.getItem(STORAGE_CURRENT_PATH) || "/";
}

// ─────────────────────────────────────────────────────────────────────────────
// Client lifecycle (one DriveClient per session, kept in sync with api/signer)
// ─────────────────────────────────────────────────────────────────────────────

const client = new DriveClient();

api$$.subscribe((api) => {
  client.setApi(api);
});

// Use combineLatest so the client always receives signer + address + keypair
// together. With separate subscriptions the client is left partially updated
// between emissions (e.g. a stale keypair signs provider HTTP requests as the
// previous account, which the provider rejects with 401).
combineLatest([signer$$, signerAddress$$, keypair$$]).subscribe(
  ([signer, address, keypair]) => {
    client.setSigner(signer, address, keypair);
  },
);

export function getDriveClient(): DriveClient {
  return client;
}

// ─────────────────────────────────────────────────────────────────────────────
// State subjects
// ─────────────────────────────────────────────────────────────────────────────

const drives$ = new BehaviorSubject<DriveInfo[]>([]);
const selectedDrive$ = new BehaviorSubject<DriveInfo | null>(null);
const currentPath$ = new BehaviorSubject<string>(readCurrentPath());
const entries$ = new BehaviorSubject<FsEntry[]>([]);
const loading$ = new BehaviorSubject<boolean>(false);
const uploading$ = new BehaviorSubject<boolean>(false);
const error$ = new BehaviorSubject<string | null>(null);
const viewMode$ = new BehaviorSubject<ViewMode>(readViewMode());
const creations$ = new BehaviorSubject<CreationStatus[]>([]);
const eventTick$ = new BehaviorSubject<number>(0);

let uploadAbortController: AbortController | null = null;
let pendingSelectedDriveId: bigint | null = readSelectedDriveId();

// Persist viewMode + currentPath + selected drive id
viewMode$.subscribe((mode) => localStorage.setItem(STORAGE_VIEW_MODE, mode));
currentPath$.subscribe((path) => localStorage.setItem(STORAGE_CURRENT_PATH, path));
selectedDrive$.subscribe((d) => {
  if (d) localStorage.setItem(STORAGE_SELECTED_DRIVE, d.driveId.toString());
  else localStorage.removeItem(STORAGE_SELECTED_DRIVE);
});

// ─────────────────────────────────────────────────────────────────────────────
// Hooks
// ─────────────────────────────────────────────────────────────────────────────

export const [useDrives] = bind(drives$, []);
export const [useSelectedDrive] = bind(selectedDrive$, null);
export const [useCurrentPath] = bind(currentPath$, "/");
export const [useEntries] = bind(entries$, []);
export const [useDriveLoading] = bind(loading$, false);
export const [useUploading] = bind(uploading$, false);
export const [useDriveError] = bind(error$, null);
export const [useViewMode] = bind(viewMode$, "list");
export const [useCreations] = bind(creations$, []);

// ─────────────────────────────────────────────────────────────────────────────
// Drive CRUD
// ─────────────────────────────────────────────────────────────────────────────

export async function refreshDrives(): Promise<void> {
  if (!client.hasApi() || !client.hasSigner()) return;
  loading$.next(true);
  try {
    const list = await client.listDrives();
    drives$.next(list);

    // Reconcile selected drive with refreshed list
    const sel = selectedDrive$.getValue();
    if (sel) {
      const updated = list.find((d) => d.driveId === sel.driveId) ?? null;
      if (updated && updated.name !== sel.name) {
        selectedDrive$.next(updated);
      } else if (!updated) {
        selectedDrive$.next(null);
        entries$.next([]);
      }
    } else if (pendingSelectedDriveId !== null) {
      // Hydrate selection from localStorage on first successful refresh
      const persisted = list.find((d) => d.driveId === pendingSelectedDriveId);
      if (persisted) {
        selectedDrive$.next(persisted);
      }
      pendingSelectedDriveId = null;
    }

    error$.next(null);
  } catch (err) {
    error$.next(err instanceof Error ? err.message : "Failed to load drives");
  } finally {
    loading$.next(false);
  }
}

export async function selectDrive(drive: DriveInfo | null): Promise<void> {
  selectedDrive$.next(drive);
  if (!drive) {
    entries$.next([]);
    currentPath$.next("/");
    return;
  }
  currentPath$.next("/");
}

export function navigateTo(path: string): void {
  currentPath$.next(path);
}

export function navigateUp(): void {
  const path = currentPath$.getValue();
  if (path === "/") return;
  const parts = path.split("/").filter(Boolean);
  parts.pop();
  currentPath$.next(parts.length === 0 ? "/" : "/" + parts.join("/"));
}

export async function refreshDirectory(): Promise<void> {
  const drive = selectedDrive$.getValue();
  if (!drive || !client.hasApi()) return;
  loading$.next(true);
  error$.next(null);
  try {
    const list = await client.listDirectory(drive.bucketId, currentPath$.getValue());
    entries$.next(list);
  } catch (err) {
    error$.next(err instanceof Error ? err.message : "Failed to list directory");
    entries$.next([]);
  } finally {
    loading$.next(false);
  }
}

export function setViewMode(mode: ViewMode): void {
  viewMode$.next(mode);
}

// ─────────────────────────────────────────────────────────────────────────────
// File operations
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
  const drive = selectedDrive$.getValue();
  if (!drive || !client.hasApi()) return;
  if (files.length === 0) return;

  uploadAbortController = new AbortController();
  const signal = uploadAbortController.signal;
  uploading$.next(true);

  const uploaded: string[] = [];
  let aborted = false;

  try {
    const path = currentPath$.getValue();
    for (const file of files) {
      if (signal.aborted) {
        aborted = true;
        break;
      }
      const data = await readFileAsUint8Array(file);
      const filePath = path === "/" ? `/${file.name}` : `${path}/${file.name}`;
      await client.uploadFile(drive.bucketId, filePath, data, {
        contentType: file.type || "application/octet-stream",
        signal,
      });
      uploaded.push(file.name);
    }
  } finally {
    uploading$.next(false);
    uploadAbortController = null;
    if (uploaded.length > 0) {
      await refreshDirectory();
    }
  }

  if (aborted && uploaded.length < files.length) {
    throw new DOMException("Upload aborted", "AbortError");
  }
}

export function abortUpload(): void {
  uploadAbortController?.abort();
}

export async function downloadFile(entry: FsEntry): Promise<Blob> {
  const drive = selectedDrive$.getValue();
  if (!drive) throw new Error("No drive selected");
  return client.downloadFile(drive.bucketId, entry.path);
}

export async function deleteEntry(entry: FsEntry): Promise<void> {
  const drive = selectedDrive$.getValue();
  if (!drive) return;
  await client.deleteFile(drive.bucketId, entry.path);
  await refreshDirectory();
}

export async function createFolder(name: string): Promise<void> {
  const drive = selectedDrive$.getValue();
  if (!drive) return;
  const path = currentPath$.getValue();
  const folderPath = path === "/" ? `/${name}` : `${path}/${name}`;
  await client.createDirectory(drive.bucketId, folderPath);
  await refreshDirectory();
}

// ─────────────────────────────────────────────────────────────────────────────
// Drive lifecycle
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

async function runChainSubmit(id: string, ctx: RetryCtx): Promise<DriveInfo | null> {
  updateCreation(id, { stage: "submitting", error: undefined });
  try {
    // Name lives on the creation record (keyed by id), not the retry context.
    // Empty string is treated as "no name" by submitCreateDrive.
    const name = creations$.getValue().find((c) => c.id === id)?.name || undefined;
    const drive = await client.submitCreateDrive(name, ctx.provider.account, ctx.url, ctx.signed);
    updateCreation(id, { stage: "ready", bucketId: drive.bucketId });
    retryCtx.delete(id);
    await refreshDrives();
    const refreshed = drives$.getValue().find((d) => d.driveId === drive.driveId) ?? drive;
    await selectDrive(refreshed);
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

export async function createDrive(input: CreateDriveInput): Promise<DriveInfo | null> {
  if (!client.hasApi() || !client.hasSigner()) return null;

  const id = crypto.randomUUID();
  creations$.next([
    ...creations$.getValue(),
    // Store the raw name; the "Untitled Drive" fallback is applied at render.
    { id, name: input.name ?? "", stage: "submitting", elapsedMs: 0 },
  ]);

  // Terms are negotiated by the caller; this only does the chain submit.
  // Stash retry context first so a failure leaves a retry handle attached
  // to the CreationStatus.
  const ctx: RetryCtx = { provider: input.provider, url: input.url, signed: input.signed };
  retryCtx.set(id, ctx);
  return runChainSubmit(id, ctx);
}

/**
 * Retry a failed on-chain submit using the cached signed terms. No-op if
 * the creation expired or never negotiated successfully.
 */
export async function retryCreation(
  id: string,
  name?: string,
): Promise<DriveInfo | null> {
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
export async function queryMatchingProviders(query: QueryMatchingProvidersParams['query'], limit: QueryMatchingProvidersParams['limit'] = DEFAULT_PROVIDER_LIMIT): Promise<MatchingProviders[]> {
   if (!client.hasApi()) return [];
    return client.queryMatchingProviders(query, limit);
}


export async function deleteDrive(driveId: bigint): Promise<void> {
  if (!client.hasApi() || !client.hasSigner()) return;
  await client.deleteDrive(driveId);
  if (selectedDrive$.getValue()?.driveId === driveId) {
    selectedDrive$.next(null);
    entries$.next([]);
    currentPath$.next("/");
  }
  await refreshDrives();
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
  role: import("@/lib/drive-client").MemberRole,
): Promise<void> {
  await client.addMember(bucketId, account, role);
}

export async function removeMember(bucketId: bigint, account: string): Promise<void> {
  await client.removeMember(bucketId, account);
}

// ─────────────────────────────────────────────────────────────────────────────
// Real-time DriveRegistry event subscription
//
// Subscribed once when the api becomes available; refreshes drives whenever
// a DriveCreated / DriveDeleted event fires that affects the current signer
// (or any drive currently being tracked).
// ─────────────────────────────────────────────────────────────────────────────

let eventSub: Subscription | null = null;

function subscribeToDriveEvents(): void {
  eventSub?.unsubscribe();
  eventSub = null;
  const api = getApi();
  if (!api) return;

  // React to events that involve the current signer's drives or any drive
  // we're already tracking — otherwise ignore (e.g. another user's drive on
  // a shared chain). Coalesce bursts of events into a single refresh via
  // queueMicrotask so back-to-back events in one block don't trigger
  // multiple list reads.
  const handle = (driveId: bigint, owner: string): void => {
    const ownAddr = getSignerAddress();
    const tracked = new Set(drives$.getValue().map((d) => d.driveId));
    if (owner !== ownAddr && !tracked.has(driveId)) return;
    eventTick$.next(eventTick$.getValue() + 1);
    queueMicrotask(() => refreshDrives().catch(() => {}));
  };

  eventSub = new Subscription();
  eventSub.add(
    api.event.DriveRegistry.DriveCreated.watch().subscribe({
      next: ({ events }) => events.forEach(({ payload }) => handle(payload.drive_id, payload.owner)),
      error: () => {},
    }),
  );
  eventSub.add(
    api.event.DriveRegistry.DriveDeleted.watch().subscribe({
      next: ({ events }) => events.forEach(({ payload }) => handle(payload.drive_id, payload.owner)),
      error: () => {},
    }),
  );
}

api$$.subscribe(() => {
  subscribeToDriveEvents();
});

// ─────────────────────────────────────────────────────────────────────────────
// Reactive: refresh drives when api+signer become available; refresh directory
// when (selectedDrive, currentPath) change.
// ─────────────────────────────────────────────────────────────────────────────

combineLatest([api$$, signerAddress$$])
  .pipe(distinctUntilChanged((a, b) => a[0] === b[0] && a[1] === b[1]))
  .subscribe(([api, address]) => {
    if (api && address) {
      // Clear previous user's state before loading the new user's drives
      drives$.next([]);
      selectedDrive$.next(null);
      entries$.next([]);
      currentPath$.next("/");
      refreshDrives().catch(() => { /* swallow */ });
    } else {
      drives$.next([]);
      selectedDrive$.next(null);
      entries$.next([]);
    }
  });

combineLatest([selectedDrive$, currentPath$])
  .pipe(distinctUntilChanged((a, b) => a[0]?.driveId === b[0]?.driveId && a[1] === b[1]))
  .subscribe(([drive]) => {
    if (drive && client.hasApi()) {
      refreshDirectory().catch(() => { /* swallow; error$ surfaces it */ });
    }
  });

// ─────────────────────────────────────────────────────────────────────────────
// Non-reactive getters
// ─────────────────────────────────────────────────────────────────────────────

export function getDrives(): DriveInfo[] {
  return drives$.getValue();
}

export function getSelectedDrive(): DriveInfo | null {
  return selectedDrive$.getValue();
}

export function getCurrentPath(): string {
  return currentPath$.getValue();
}
