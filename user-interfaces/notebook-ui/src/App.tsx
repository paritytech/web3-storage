// SPDX-License-Identifier: GPL-3.0-only

import { useCallback, useEffect, useMemo, useState } from "react";
import { ConnectionBar } from "@/components/ConnectionBar";
import { FileList } from "@/components/FileList";
import { Editor } from "@/components/Editor";
import { HistoryPanel } from "@/components/HistoryPanel";
import { NOTEBOOK_ABI, NOTEBOOK_BYTECODE } from "@/lib/contract";
import { connectChain, deriveSigner } from "@/lib/papi";
import { NotebookClient, toHex } from "@/lib/notebook";

const STORAGE_KEY = "web3-notebook-ui:v1";
const UNIT = 10n ** 12n;

interface PersistedState {
  chainWs: string;
  providerUrl: string;
  signerSeed: string;
  providerSeed: string;
  contractAddress: string;
  s3BucketId: string;
}

function loadPersisted(): PersistedState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return { ...defaults(), ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return defaults();
}

function defaults(): PersistedState {
  return {
    chainWs: "ws://127.0.0.1:2222",
    providerUrl: "http://127.0.0.1:3333",
    signerSeed: "//Bob",
    providerSeed: "//Alice",
    contractAddress: "",
    s3BucketId: "",
  };
}

interface FileEntry {
  /** keccak256(key) hex, used for indexed-event filtering. */
  keyHash: string;
  key: string;
  /** Per-tx event batches in arrival order; replayed for history. */
  eventBatches: unknown[][];
}

export default function App() {
  const [persisted, setPersisted] = useState<PersistedState>(loadPersisted);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const [notebook, setNotebook] = useState<NotebookClient | null>(null);

  const [files, setFiles] = useState<Record<string, FileEntry>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [editorContent, setEditorContent] = useState("");
  const [commitMessage, setCommitMessage] = useState("");
  const [viewingCid, setViewingCid] = useState<string | null>(null);
  const [viewingRevision, setViewingRevision] = useState<number | null>(null);
  const [viewingContent, setViewingContent] = useState<string | null>(null);

  // Persist UI prefs whenever they change.
  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(persisted));
  }, [persisted]);

  const update = <K extends keyof PersistedState>(key: K, value: PersistedState[K]) =>
    setPersisted((p) => ({ ...p, [key]: value }));

  const ensureAccountMapped = useCallback(
    async (api: any, signer: any) => {
      console.log("[map_account] submitting…");
      try {
        const tx = api.tx.Revive.map_account();
        await new Promise<void>((resolve, reject) => {
          const timer = setTimeout(
            () => reject(new Error("map_account timed out after 60s")),
            60_000,
          );
          let sub: { unsubscribe: () => void } | null = null;
          sub = tx.signSubmitAndWatch(signer).subscribe({
            next: (ev: any) => {
              console.log("[map_account] event:", ev);
              if (ev.type === "txBestBlocksState" && ev.found) {
                clearTimeout(timer);
                sub?.unsubscribe();
                resolve();
              }
            },
            error: (err: any) => {
              clearTimeout(timer);
              console.warn("[map_account] error:", err);
              sub?.unsubscribe();
              const msg = String(err?.message ?? err);
              if (msg.includes("AlreadyMapped")) resolve();
              else reject(err);
            },
          });
        });
      } catch (err) {
        const msg = String((err as Error)?.message ?? err);
        if (!msg.includes("AlreadyMapped")) throw err;
      }
    },
    [],
  );

  // ── Deploy / Attach ────────────────────────────────────────────────────────

  const handleDeploy = useCallback(async () => {
    setBusy(true);
    setStatus("Connecting…");
    try {
      const { api } = connectChain(persisted.chainWs);
      const author = deriveSigner(persisted.signerSeed);
      const provider = deriveSigner(persisted.providerSeed);
      setStatus("Mapping account…");
      await ensureAccountMapped(api, author.signer);
      setStatus("Deploying contract + opening bucket (this takes a few seconds)…");
      const client = await NotebookClient.deploy({
        api,
        signer: author.signer,
        providerUrl: persisted.providerUrl,
        providerPublicKey: toHex(provider.publicKey),
        abi: NOTEBOOK_ABI,
        bytecode: NOTEBOOK_BYTECODE,
        bucketName: `notebook-${Date.now().toString(36)}`,
        maxBytes: 1n << 20n,
        duration: 50,
        pricePerByte: 1n,
        value: 5n * UNIT,
      });
      setNotebook(client);
      setPersisted((p) => ({
        ...p,
        contractAddress: client.address,
        s3BucketId: client.s3BucketId.toString(),
      }));
      setStatus(
        `Deployed at ${client.address} (bucket ${client.s3BucketId}). Create a file to start.`,
      );
    } catch (err) {
      setStatus(`Deploy failed: ${String((err as Error)?.message ?? err)}`);
    } finally {
      setBusy(false);
    }
  }, [persisted, ensureAccountMapped]);

  const handleAttach = useCallback(async () => {
    setBusy(true);
    setStatus("Attaching…");
    try {
      const { api } = connectChain(persisted.chainWs);
      const author = deriveSigner(persisted.signerSeed);
      await ensureAccountMapped(api, author.signer);
      const client = NotebookClient.attach({
        api,
        signer: author.signer,
        providerUrl: persisted.providerUrl,
        abi: NOTEBOOK_ABI,
        address: persisted.contractAddress as `0x${string}`,
        s3BucketId: BigInt(persisted.s3BucketId),
      });
      setNotebook(client);
      setStatus(`Attached to ${client.address}.`);
    } catch (err) {
      setStatus(`Attach failed: ${String((err as Error)?.message ?? err)}`);
    } finally {
      setBusy(false);
    }
  }, [persisted, ensureAccountMapped]);

  const handleDisconnect = useCallback(() => {
    setNotebook(null);
    setFiles({});
    setSelected(null);
    setEditorContent("");
    setStatus("");
  }, []);

  // ── File operations ────────────────────────────────────────────────────────

  const ensureFileEntry = (key: string): FileEntry =>
    files[key] ?? {
      keyHash: "",
      key,
      eventBatches: [],
    };

  const handleNewFile = useCallback(() => {
    const key = prompt("File name (e.g. 'hack.md')")?.trim();
    if (!key) return;
    if (files[key]) {
      setSelected(key);
      setStatus(`'${key}' already exists.`);
      return;
    }
    setFiles((f) => ({
      ...f,
      [key]: { keyHash: "", key, eventBatches: [] },
    }));
    setSelected(key);
    setEditorContent("");
    setCommitMessage("");
    setViewingCid(null);
    setViewingContent(null);
    setViewingRevision(null);
    setStatus(`New file '${key}'. Type, then click Create.`);
  }, [files]);

  const handleSelect = useCallback(
    async (key: string) => {
      setSelected(key);
      setCommitMessage("");
      setViewingCid(null);
      setViewingContent(null);
      setViewingRevision(null);
      setStatus("");
      const entry = files[key];
      if (!entry || !notebook) return;
      if (entry.eventBatches.length === 0) {
        setEditorContent("");
        return;
      }
      setBusy(true);
      try {
        const bytes = await notebook.fetchCurrentBytes(key);
        setEditorContent(new TextDecoder().decode(bytes));
      } catch (err) {
        setStatus(`Fetch failed: ${String((err as Error)?.message ?? err)}`);
      } finally {
        setBusy(false);
      }
    },
    [files, notebook],
  );

  const currentRevision = useMemo(() => {
    if (!selected) return null;
    const entry = files[selected];
    if (!entry) return null;
    const history = notebook?.historyFromBatches(entry.eventBatches, selected) ?? [];
    let rev = 0;
    for (const e of history) {
      if (e.eventName === "FileCreated") rev = 1;
      else if (e.eventName === "FileUpdated") rev = Number(e.args.newRevision);
    }
    return rev || null;
  }, [selected, files, notebook]);

  const isNew = currentRevision === null;

  const handleSave = useCallback(async () => {
    if (!notebook || !selected) return;
    setBusy(true);
    const bytes = new TextEncoder().encode(editorContent);
    try {
      if (isNew) {
        setStatus(`Creating '${selected}'…`);
        const r = await notebook.createFile(selected, bytes, "text/markdown");
        setFiles((f) => {
          const e = f[selected] ?? { keyHash: "", key: selected, eventBatches: [] };
          return {
            ...f,
            [selected]: { ...e, eventBatches: [...e.eventBatches, r.events] },
          };
        });
        setStatus(`Created rev 1 (${r.cid.slice(0, 10)}…).`);
      } else {
        setStatus(`Saving rev ${currentRevision! + 1}…`);
        const r = await notebook.updateFile(
          selected,
          bytes,
          "text/markdown",
          currentRevision!,
          commitMessage || "(no message)",
        );
        setFiles((f) => {
          const e = f[selected]!;
          return {
            ...f,
            [selected]: { ...e, eventBatches: [...e.eventBatches, r.events] },
          };
        });
        setStatus(`Saved rev ${r.revision} (${r.cid.slice(0, 10)}…).`);
      }
      setCommitMessage("");
      setViewingCid(null);
      setViewingContent(null);
      setViewingRevision(null);
    } catch (err) {
      setStatus(`Save failed: ${String((err as Error)?.message ?? err)}`);
    } finally {
      setBusy(false);
    }
  }, [notebook, selected, editorContent, isNew, currentRevision, commitMessage]);

  const handleView = useCallback(
    async (cid: `0x${string}`, revision: number) => {
      if (!notebook) return;
      setBusy(true);
      setStatus(`Fetching rev ${revision}…`);
      try {
        const bytes = await notebook.fetchBytesByCid(cid);
        setViewingContent(new TextDecoder().decode(bytes));
        setViewingCid(cid);
        setViewingRevision(revision);
        setStatus(
          `Viewing rev ${revision} as diff vs current — green will return, red will go away on revert.`,
        );
      } catch (err) {
        setStatus(`View failed: ${String((err as Error)?.message ?? err)}`);
      } finally {
        setBusy(false);
      }
    },
    [notebook],
  );

  const handleCancelViewing = useCallback(() => {
    setViewingCid(null);
    setViewingRevision(null);
    setViewingContent(null);
    setStatus("");
  }, []);

  const handleRevert = useCallback(
    async (cid: `0x${string}`, revision: number) => {
      if (!notebook || !selected || currentRevision === null) return;
      setBusy(true);
      setStatus(`Reverting to rev ${revision}…`);
      try {
        const bytes = await notebook.fetchBytesByCid(cid);
        const r = await notebook.updateFile(
          selected,
          bytes,
          "text/markdown",
          currentRevision,
          `Reverted to revision ${revision}`,
        );
        setFiles((f) => {
          const e = f[selected]!;
          return {
            ...f,
            [selected]: { ...e, eventBatches: [...e.eventBatches, r.events] },
          };
        });
        setEditorContent(new TextDecoder().decode(bytes));
        setViewingCid(null);
        setViewingRevision(null);
        setViewingContent(null);
        setStatus(`Reverted to rev ${revision} (now rev ${r.revision}).`);
      } catch (err) {
        setStatus(`Revert failed: ${String((err as Error)?.message ?? err)}`);
      } finally {
        setBusy(false);
      }
    },
    [notebook, selected, currentRevision],
  );

  // ── Derived view state ─────────────────────────────────────────────────────

  const history = useMemo(() => {
    if (!notebook || !selected) return [];
    const entry = files[selected];
    if (!entry) return [];
    return notebook.historyFromBatches(entry.eventBatches, selected);
  }, [notebook, files, selected]);

  const fileKeys = useMemo(() => Object.keys(files), [files]);

  return (
    <div className="flex h-screen flex-col">
      <ConnectionBar
        chainWs={persisted.chainWs}
        providerUrl={persisted.providerUrl}
        signerSeed={persisted.signerSeed}
        providerSeed={persisted.providerSeed}
        contractAddress={persisted.contractAddress}
        s3BucketId={persisted.s3BucketId}
        status={status}
        busy={busy}
        connected={notebook != null}
        onChainWsChange={(v) => update("chainWs", v)}
        onProviderUrlChange={(v) => update("providerUrl", v)}
        onSignerSeedChange={(v) => update("signerSeed", v)}
        onProviderSeedChange={(v) => update("providerSeed", v)}
        onContractAddressChange={(v) => update("contractAddress", v)}
        onS3BucketIdChange={(v) => update("s3BucketId", v)}
        onDeploy={handleDeploy}
        onAttach={handleAttach}
        onDisconnect={handleDisconnect}
      />
      {notebook ? (
        <div className="flex flex-1 overflow-hidden">
          <FileList
            files={fileKeys}
            selected={selected}
            onSelect={handleSelect}
            onNew={handleNewFile}
          />
          <Editor
            fileKey={selected}
            content={editorContent}
            commitMessage={commitMessage}
            busy={busy}
            isNew={isNew}
            currentRevision={currentRevision}
            readOnlyNotice={
              viewingCid != null
                ? `Diff vs rev ${viewingRevision} — green will return, red will go away if you revert.`
                : null
            }
            viewingContent={viewingContent}
            onContentChange={setEditorContent}
            onCommitMessageChange={setCommitMessage}
            onSave={handleSave}
            onCancelViewing={handleCancelViewing}
          />
          <HistoryPanel
            fileKey={selected}
            history={history}
            viewingCid={viewingCid}
            busy={busy}
            onView={handleView}
            onRevert={handleRevert}
          />
        </div>
      ) : (
        <div className="flex flex-1 items-center justify-center text-muted-foreground">
          Configure chain + provider above, then Deploy or Attach a notebook.
        </div>
      )}
    </div>
  );
}

