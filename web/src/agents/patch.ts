// Unified-diff text (from `git diff`) → rows for the Code view.

export interface PatchLine {
  op: " " | "+" | "-";
  text: string;
}

/** Hunk bodies only: file headers and "\ No newline" markers are dropped, hunk headers kept as context. */
export function patchLines(patch: string): PatchLine[] {
  const out: PatchLine[] = [];
  let inHunk = false;
  for (const l of patch.split("\n")) {
    if (l.startsWith("@@")) {
      inHunk = true;
      out.push({ op: " ", text: l });
    } else if (!inHunk || l.startsWith("\\")) {
      continue;
    } else if (l.startsWith("+")) out.push({ op: "+", text: l.slice(1) });
    else if (l.startsWith("-")) out.push({ op: "-", text: l.slice(1) });
    else if (l.startsWith(" ")) out.push({ op: " ", text: l.slice(1) });
    else if (l.startsWith("diff ")) inHunk = false;
  }
  return out;
}
