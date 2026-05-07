/**
 * Drive State - file system orchestration over a DriveClient.
 *
 * Owns the DriveClient instance, syncing it with chain.state.ts (api) and
 * wallet.state.ts (signer). Holds drives/entries/selection state. Subscribes
 * to DriveRegistry events for real-time updates.
 */

import { BehaviorSubject, combineLatest, distinctUntilChanged } from "rxjs";
import { bind } from "@react-rxjs/core";
import {
  DriveClient,
  type DriveInfo,
  type FsEntry,
  type CreateDriveOptions,
} from "@/lib/drive-client";
import { api$$, getApi } from "@/state/chain.state";
import { signer$$, getSignerAddress, refreshBalance } from "@/state/wallet.state";

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

export type CreationStage = "submitting" | "created" | "waiting" | "ready" | "failed";

export interface CreationStatus {
  id: string;
  name: string;
  stage: CreationStage;
  statusMessage?: string;
  elapsedMs: number;
  error?: string;
  bucketId?: bigint;
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

signer$$.subscribe((signer) => {
  client.setSigner(signer, getSignerAddress());
});

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

export async function createDrive(options: CreateDriveOptions): Promise<DriveInfo | null> {
  if (!client.hasApi() || !client.hasSigner()) return null;

  const id = crypto.randomUUID();
  const name = options.name || "Untitled Drive";
  creations$.next([
    ...creations$.getValue(),
    { id, name, stage: "submitting", elapsedMs: 0 },
  ]);

  try {
    const drive = await client.createDrive(options);
    updateCreation(id, { stage: "created", bucketId: drive.bucketId });

    updateCreation(id, { stage: "waiting" });
    try {
      await client.waitForProvider(drive.bucketId, (status, elapsedMs) => {
        updateCreation(id, { statusMessage: status, elapsedMs });
      });
      updateCreation(id, { stage: "ready" });
      await refreshDrives();
      const refreshed = drives$.getValue().find((d) => d.driveId === drive.driveId) ?? drive;
      await selectDrive(refreshed);
      await refreshBalance();
      return refreshed;
    } catch (err) {
      updateCreation(id, {
        stage: "failed",
        error: err instanceof Error ? err.message : "Provider did not accept",
      });
      await refreshDrives();
      return null;
    }
  } catch (err) {
    updateCreation(id, {
      stage: "failed",
      error: err instanceof Error ? err.message : "Failed to create drive",
    });
    return null;
  }
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

interface RegistryEventSubscription {
  unsubscribe: () => void;
}

let eventSub: RegistryEventSubscription | null = null;

function subscribeToDriveEvents(): void {
  if (eventSub) {
    eventSub.unsubscribe();
    eventSub = null;
  }
  const api = getApi();
  if (!api) return;

  // polkadot-api's typed event watcher delivers the decoded payload at the top
  // of the emitted object (no `value` wrapper) — but the precise shape can
  // vary between runtime versions, so accept whatever comes and pluck fields
  // defensively.
  type EventObserver = {
    next: (ev: unknown) => void;
    error?: (err: unknown) => void;
  };
  type EventWatcher = {
    watch: () => {
      subscribe: (
        handler: ((ev: unknown) => void) | EventObserver,
      ) => RegistryEventSubscription;
    };
  };
  const events = (api as unknown as {
    event: { DriveRegistry: Record<string, EventWatcher> };
  }).event.DriveRegistry;

  const interesting = ["DriveCreated", "DriveDeleted"];

  const subscriptions: RegistryEventSubscription[] = [];

  for (const name of interesting) {
    const watcher = events[name];
    if (!watcher) continue;
    try {
      const sub = watcher.watch().subscribe({
        next: (tick: unknown) => {
          if (!tick || typeof tick !== "object") return;
          // polkadot-api 2.x delivers `{ block, events: PalletEvent<T>[] }`
          // per tick; each PalletEvent has a `payload` with the decoded
          // data. Tolerate both shapes (older versions emitted single
          // events with payload-as-self) so we don't break on a runtime
          // upgrade.
          const tickEvents = (tick as { events?: unknown }).events;
          const items: Array<{ payload?: unknown }> = Array.isArray(tickEvents)
            ? (tickEvents as Array<{ payload?: unknown }>)
            : [tick as { payload?: unknown }];

          const ownAddr = getSignerAddress();
          const tracked = new Set(drives$.getValue().map((d) => d.driveId));

          for (const item of items) {
            const payload = ((item.payload ?? item) as {
              drive_id?: bigint | number;
              owner?: string;
            });
            if (!payload || typeof payload !== "object") continue;

            const driveIdRaw = payload.drive_id;
            const driveId =
              typeof driveIdRaw === "bigint"
                ? driveIdRaw
                : typeof driveIdRaw === "number"
                  ? BigInt(driveIdRaw)
                  : undefined;

            // Only react to events involving the current signer or already-
            // tracked drives. `owner` is absent on some event variants; in
            // that case fall back to "we track this drive_id".
            const isOwn = ownAddr && payload.owner === ownAddr;
            const isTracked = driveId !== undefined && tracked.has(driveId);

            if (isOwn || isTracked) {
              eventTick$.next(eventTick$.getValue() + 1);
              // Coalesce bursts of events into one refresh.
              queueMicrotask(() => {
                refreshDrives().catch(() => { /* swallow */ });
              });
              return;
            }
          }
        },
        // Some events listed in the descriptor may not exist on the running
        // runtime ("Runtime entry Event(DriveRegistry.X) not found"). Swallow
        // — that event variant just isn't deliverable here.
        error: () => { /* swallow */ },
      });
      subscriptions.push(sub);
    } catch {
      // event name not in current runtime version; skip
    }
  }

  eventSub = {
    unsubscribe: () => subscriptions.forEach((s) => s.unsubscribe()),
  };
}

api$$.subscribe(() => {
  subscribeToDriveEvents();
});

// ─────────────────────────────────────────────────────────────────────────────
// Reactive: refresh drives when api+signer become available; refresh directory
// when (selectedDrive, currentPath) change.
// ─────────────────────────────────────────────────────────────────────────────

combineLatest([api$$, signer$$])
  .pipe(distinctUntilChanged())
  .subscribe(([api, signer]) => {
    if (api && signer) {
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
