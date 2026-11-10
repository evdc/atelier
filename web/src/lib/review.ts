import type { SessionRow } from "../gen/SessionRow";

/** How a new session's workspace comes from another session: read-only, or a branch at a snapshot. */
export type Source = "share" | "branch";

/** What the new-session form starts with, handed over by "Review changes". */
export type Prefill = { source: Source; from: string; title: string; prompt: string };

/** The review prompt lives here, so `spawn` and automations can reuse it. */
export function reviewPrompt(s: { title: string; base?: string | null; branch?: string | null }): string {
  return (
    `Review the changes in this worktree (base \`${s.base}\`, branch \`${s.branch}\`) made by session "${s.title}". ` +
    `Read the diff with \`git diff ${s.base}\`, run the tests, and report problems by severity with file:line. Don't edit files.`
  );
}

export const reviewTitle = (title: string) => `Review: ${title}`;

/** Live sessions reading `owner`'s worktree: shared from it and not archived. */
export function readersOf(all: SessionRow[], owner: string): SessionRow[] {
  return all.filter((s) => !s.archived && s.workspace.mode === "shared" && s.workspace.from === owner);
}

/** A commit's short form, for provenance lines. */
export const shortCommit = (sha: string) => sha.slice(0, 7);

// Handed from the session view to the new-session form, which takes it once when it mounts.
let pending: Prefill | null = null;

export function prefillNewSession(p: Prefill): void {
  pending = p;
}

export function takePrefill(): Prefill | null {
  const p = pending;
  pending = null;
  return p;
}
