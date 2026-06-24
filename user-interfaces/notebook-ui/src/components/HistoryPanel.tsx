// SPDX-License-Identifier: GPL-3.0-only

import type { FileEvent } from "@/lib/notebook";

interface Row {
  revision: number;
  cid: `0x${string}`;
  message: string;
  isLatest: boolean;
}

interface Props {
  fileKey: string | null;
  history: FileEvent[];
  viewingCid: string | null;
  busy: boolean;
  onView: (cid: `0x${string}`, revision: number) => void;
  onRevert: (cid: `0x${string}`, revision: number) => void;
}

function eventToRow(e: FileEvent, isLatest: boolean): Row {
  if (e.eventName === "FileCreated") {
    return {
      revision: 1,
      cid: e.args.cid as `0x${string}`,
      message: "initial",
      isLatest,
    };
  }
  return {
    revision: Number(e.args.newRevision),
    cid: e.args.newCid as `0x${string}`,
    message: (e.args.commitMessage as string) || "(no message)",
    isLatest,
  };
}

export function HistoryPanel(props: Props) {
  if (!props.fileKey) {
    return <aside className="w-80 border-l border-border bg-card" />;
  }
  const rows: Row[] = props.history
    .filter((e) => e.eventName !== "FileDeleted")
    .map((e, i, arr) => eventToRow(e, i === arr.length - 1));
  // Newest first.
  rows.reverse();

  return (
    <aside className="flex w-80 flex-col border-l border-border bg-card">
      <header className="border-b border-border p-3">
        <h2 className="text-sm font-medium">History</h2>
        <p className="text-xs text-muted-foreground">{rows.length} revision{rows.length === 1 ? "" : "s"}</p>
      </header>
      <div className="flex-1 overflow-y-auto">
        {rows.length === 0 ? (
          <p className="p-3 text-xs text-muted-foreground">
            Save the file to start the history.
          </p>
        ) : (
          rows.map((row) => {
            const viewing = props.viewingCid === row.cid;
            return (
              <div
                key={`${row.revision}-${row.cid}`}
                className={`border-b border-border p-3 text-xs ${viewing ? "bg-muted" : ""}`}
              >
                <div className="flex items-baseline justify-between">
                  <span className="font-medium">rev {row.revision}</span>
                  {row.isLatest && (
                    <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                      current
                    </span>
                  )}
                </div>
                <div className="mt-0.5 break-all font-mono text-[10px] text-muted-foreground">
                  {row.cid}
                </div>
                <div className="mt-1 italic">{row.message}</div>
                <div className="mt-2 flex gap-1">
                  <button
                    type="button"
                    onClick={() => props.onView(row.cid, row.revision)}
                    disabled={props.busy || viewing}
                    className="flex-1 rounded border border-border bg-background px-2 py-1 disabled:opacity-50"
                  >
                    {viewing ? "Viewing" : "View"}
                  </button>
                  {!row.isLatest && (
                    <button
                      type="button"
                      onClick={() => props.onRevert(row.cid, row.revision)}
                      disabled={props.busy}
                      className="flex-1 rounded border border-border bg-background px-2 py-1 disabled:opacity-50"
                    >
                      Revert
                    </button>
                  )}
                </div>
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
}
