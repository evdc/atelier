# Tickets

One file per work item: front matter, then **Context → Goal → Scope → Design notes → Acceptance → Out of scope**.
Written to be picked up cold by an agent: read the ticket, then the files it names, then `CLAUDE.md`.

`status`: open | in-progress | done. `size`: S (≤ half a day), M (≤ 2 days), L (split it first).
`depends`: tickets that must land first. Milestones are in `DESIGN.md` §9.

| id | title | ms | size | depends |
|---|---|---|---|---|
| AT-001 | Markdown in the transcript | M1 | M | |
| AT-002 | Long transcripts stay smooth (virtualization) | M1 | M | AT-001 |
| AT-003 | Rich tool views: diffs, output, highlighting | M1 | M | AT-001 |
| AT-004 | Import Claude Code history for added projects | M1 | M | |
| AT-005 | Reap idle agent processes | M1 | S | |
| AT-006 | Blob store for large payloads | M1 | M | |
| AT-007 | "Always allow" and deny-with-reason | M1 | S | |
| AT-008 | Usage, limits and context in the session header | M1 | S | |
| AT-009 | Keyboard: palette, shortcuts, focus | M1 | M | |
| AT-010 | Worktree lifecycle: changes, cleanup, open-in | M1 | M | |
| AT-011 | Run atelierd as a login agent; bundle it | M2 | M | |
| AT-012 | Remote access over Tailscale + pairing | M2 | M | AT-011 |
| AT-013 | iOS client | M2 | L | AT-012 |
| AT-014 | Notify when a session needs you | M2 | S | |
| AT-015 | Instant cold start (client event cache) | M2 | M | |
| AT-016 | Codex driver | M3 | L | |
| AT-017 | Session titles (rename, agent-written) | M1 | S | |
| AT-018 | Add-project: browse the server's directories | M1 | S | |
| AT-019 | CI | — | S | |
| AT-020 | Store benchmarks and the prefix-scan index | — | S | |
