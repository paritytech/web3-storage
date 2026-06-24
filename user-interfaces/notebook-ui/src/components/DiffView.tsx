// SPDX-License-Identifier: GPL-3.0-only

import { diffLines } from "diff";

interface Props {
  currentContent: string;
  historicalContent: string;
}

/** Line-level diff between the file's current bytes and the historical
 * revision being viewed. The colour semantics match "what would change if
 * I reverted to this revision":
 *
 *   green '+' = present in historical, missing from current → comes back
 *   red   '-' = present in current, missing from historical → goes away
 *   plain     = unchanged in both
 */
export function DiffView({ currentContent, historicalContent }: Props) {
  const parts = diffLines(currentContent, historicalContent);

  return (
    <div className="flex-1 overflow-auto bg-background p-2">
      <pre className="font-mono text-sm leading-relaxed">
        {parts.flatMap((part, i) => {
          const lines = part.value.replace(/\n$/, "").split("\n");
          const cls = part.added
            ? "bg-green-100 text-green-900"
            : part.removed
              ? "bg-red-100 text-red-900"
              : "text-foreground";
          const prefix = part.added ? "+ " : part.removed ? "- " : "  ";
          return lines.map((line, j) => (
            <div key={`${i}-${j}`} className={`${cls} px-2`}>
              <span className="select-none opacity-60">{prefix}</span>
              {line || " " /* keep empty lines visible */}
            </div>
          ));
        })}
      </pre>
    </div>
  );
}
