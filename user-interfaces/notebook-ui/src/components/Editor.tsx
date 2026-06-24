// SPDX-License-Identifier: GPL-3.0-only

import { DiffView } from "./DiffView";

interface Props {
  fileKey: string | null;
  content: string;
  commitMessage: string;
  busy: boolean;
  isNew: boolean;
  currentRevision: number | null;
  readOnlyNotice: string | null;
  /** When non-null, render a diff (current ↔ this) instead of the editor. */
  viewingContent: string | null;
  onContentChange: (s: string) => void;
  onCommitMessageChange: (s: string) => void;
  onSave: () => void;
  onCancelViewing: () => void;
}

export function Editor(props: Props) {
  if (!props.fileKey) {
    return (
      <main className="flex flex-1 items-center justify-center text-muted-foreground">
        Select or create a file to start editing.
      </main>
    );
  }
  const readOnly = props.readOnlyNotice != null;
  return (
    <main className="flex flex-1 flex-col">
      <header className="flex items-baseline justify-between border-b border-border bg-card px-4 py-2">
        <div className="font-mono text-sm">
          {props.fileKey}
          {props.currentRevision !== null && (
            <span className="ml-2 text-xs text-muted-foreground">
              · rev {props.currentRevision}
            </span>
          )}
          {props.isNew && (
            <span className="ml-2 rounded bg-primary px-1.5 py-0.5 text-xs text-primary-foreground">
              new
            </span>
          )}
        </div>
      </header>
      {props.readOnlyNotice && (
        <div className="flex items-center justify-between border-b border-border bg-muted px-4 py-2 text-xs">
          <span>{props.readOnlyNotice}</span>
          <button
            type="button"
            onClick={props.onCancelViewing}
            className="rounded border border-border bg-background px-2 py-1"
          >
            Back to current
          </button>
        </div>
      )}
      {props.viewingContent != null ? (
        <DiffView
          currentContent={props.content}
          historicalContent={props.viewingContent}
        />
      ) : (
        <textarea
          value={props.content}
          readOnly={readOnly}
          onChange={(e) => props.onContentChange(e.target.value)}
          className="flex-1 resize-none border-0 bg-background p-4 font-mono text-sm outline-none disabled:opacity-50"
          placeholder="Start typing…"
        />
      )}
      <footer className="flex items-center gap-2 border-t border-border bg-card p-3">
        <input
          type="text"
          value={props.commitMessage}
          onChange={(e) => props.onCommitMessageChange(e.target.value)}
          placeholder={props.isNew ? "(initial) — optional" : "Commit message"}
          disabled={readOnly}
          className="flex-1 rounded border border-border bg-background px-3 py-2 text-sm disabled:opacity-50"
        />
        <button
          type="button"
          onClick={props.onSave}
          disabled={props.busy || readOnly}
          className="rounded bg-primary px-4 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
        >
          {props.busy ? "Saving…" : props.isNew ? "Create" : "Save"}
        </button>
      </footer>
    </main>
  );
}
