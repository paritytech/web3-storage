// SPDX-License-Identifier: GPL-3.0-only

interface Props {
  files: string[];
  selected: string | null;
  onSelect: (key: string) => void;
  onNew: () => void;
}

export function FileList({ files, selected, onSelect, onNew }: Props) {
  return (
    <aside className="flex w-56 flex-col border-r border-border bg-card">
      <div className="border-b border-border p-3">
        <button
          type="button"
          onClick={onNew}
          className="w-full rounded bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground"
        >
          + New file
        </button>
      </div>
      <div className="flex-1 overflow-y-auto">
        {files.length === 0 ? (
          <div className="p-3 text-xs text-muted-foreground">
            No files yet. Create one to start.
          </div>
        ) : (
          files.map((key) => (
            <button
              key={key}
              type="button"
              onClick={() => onSelect(key)}
              className={`block w-full px-3 py-2 text-left text-sm hover:bg-muted ${
                key === selected ? "bg-muted font-medium" : ""
              }`}
            >
              {key}
            </button>
          ))
        )}
      </div>
    </aside>
  );
}
