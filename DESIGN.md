# Atelier — Design

Status: v2 (2026-11-07). Foundations implemented (M0 + most of M1). Decisions marked **[decided]** were agreed in discussion; **[proposed]** are open.
Work items live in `.atelier/tickets/`; agent instructions in `CLAUDE.md`.

See `PROMPT.md` for goals and `atelier-mockups.html` for the product vision (steps 0–8, pattern language).

---

## 1. Principles

- **Behaviour is a function of history.** Everything that happens is an immutable event; all state is a projection (`state = fold(events)`).
- **Bring your own agent.** We drive the CLIs (Claude Code first, Codex later) as subprocesses. Atelier never calls a model itself — until we add our own harness behind the same interface.
- **Plain files for what's authored.** Tickets, notes, pages, trails, proposal bodies live in `.atelier/` in the repo, synced by git.
- **Do less.** Performance comes from not doing work: don't persist ephemera, don't send clients what they didn't ask for, don't re-render what didn't change.

## 2. Source of truth **[decided]**

| Kind | Truth | Examples |
|---|---|---|
| What *happens* | Event log (SQLite, server) | messages, tool calls/results, permission requests, questions & answers, checkpoints, decisions, session lifecycle |
| What's *authored* | Files in `.atelier/` (git) | tickets, notes, pages, trails, proposals |
| Everything derived | Projections (versioned, rebuildable caches) | session list, inbox, link/backlink index, digest |

The server watches `.atelier/`; file changes are appended as events, so the log remains a complete history.

## 3. Topology **[decided]**

- Agents and repos live on the **server** (`atelierd`). Clients are pure views + input.
- Default: the desktop app starts the server locally. The phone reaches it over Tailscale.
- One server per client at a time (no multi-server federation). Addresses still carry no host so they stay portable.
- Single user, multiple devices. Multiplayer later (git-synced `.atelier/` gets us partway for free).
- **[decided] Projects.** A project is a directory (usually a git repo) explicitly added to Atelier. There is one
  cross-project view (Projects: overview + "waiting on you"); every other view (sessions, later tickets, pages,
  proposals) is scoped to a project. Only sessions in added projects are imported from CC history (AT-004).
- **[decided] Workspaces.** Each new session gets its own git worktree by default (`~/.atelier/worktrees/<project>/<id>`,
  branch `atelier/<slug>-<id>`); the new-session form can choose the project directory instead.

**[decided] Server as its own process, not in-process with the UI.** The app launches `atelierd` detached (later: as a launchd agent, AT-011) and connects to it over the same WebSocket protocol the phone uses. Consequences:
- Closing the desktop window doesn't kill running agents — essential if the phone is meant to keep working.
- Desktop and phone are the *same kind* of client: one code path, one protocol, always tested.
- Cost: a tiny bit of local IPC latency (sub-ms over loopback) — negligible.

## 4. Architecture

```
┌──────────────────── atelierd (Rust) ─────────────────────┐
│ store     SQLite WAL, single writer task                 │
│           events · blobs (content-addressed) · projections│
│ drivers   trait AgentDriver → ClaudeCode │ Codex │ Own    │
│ sessions  process supervisor, worktrees, checkpoints     │
│ watcher   .atelier/ files, ~/.claude/projects (import)   │
│ mcp       Atelier MCP server (later: ask/handoff/spawn)  │
│ sync      WebSocket: subscribe(stream, from_seq) + rpc   │
└───────────────────────────▲──────────────────────────────┘
                            │ WS (loopback or Tailscale)
               Desktop (Tauri + Solid)   iOS (Tauri + Solid)
```

### Cargo workspace

```
crates/
  atelier-core      event types, addresses, projections (pure, no IO)
  atelier-store     SQLite event store + blob store + projection runner
  atelier-proto     wire protocol types (serde), shared with client via ts-rs
  atelier-driver    AgentDriver trait + claude-code impl (codex later)
  atelier-server    atelierd binary: supervisor, sync, watcher
app/                Tauri v2 shell (desktop; iOS target later)
web/                Solid frontend (Vite), shared by desktop & mobile
```

## 5. Event log

```sql
events(
  seq     INTEGER PRIMARY KEY,   -- global, monotonic; the sync cursor
  stream  TEXT NOT NULL,         -- 'global' | 'session/<id>' | 'repo/<id>'
  ts      INTEGER NOT NULL,      -- unix ms
  kind    TEXT NOT NULL,         -- e.g. 'session.started', 'cc.assistant'
  actor   TEXT NOT NULL,         -- 'user' | 'agent:<session>' | 'atelier'
  cause   INTEGER,               -- seq of causing event, if any
  body    BLOB NOT NULL          -- JSON (later maybe msgpack)
)
blobs(hash TEXT PRIMARY KEY, bytes BLOB)   -- large tool output, diffs, file snapshots
```

**[decided] Store facts as observed; interpret in projections.**
- Agent output is stored as the vendor's own message JSON (`kind = 'cc.<type>'`), lightly wrapped — *not* normalized at ingest. A projection normalizes it into our view model (turns, tool calls, questions). Improving normalization = bumping the projection version and refolding; no data is lost or migrated.
- Our own events (`session.created`, `user.message`, `permission.requested`, `permission.answered`, `checkpoint.created`, …) use our schema.
- Large payloads (>~8 KB) move to `blobs`; the event holds the hash.

**Ephemera are not persisted.** Token deltas (`stream_event`), `thinking_tokens`, `status` are relayed live to subscribed clients and dropped. The completed assistant message is persisted. This keeps the log ~10–50× smaller and writes cheap.

**Projections** run in the writer task, updated incrementally in the same transaction as the event append (so a projection is never behind its log). Each has a version; on mismatch at startup it is rebuilt from `seq 0`.

## 6. Claude Code driver — spike results (2026-11-07, CC 2.1.319)

Verified in `spikes/claude-stream-json/`:

```
claude -p --input-format stream-json --output-format stream-json --verbose
       --include-partial-messages
       --permission-prompt-tool stdio --permission-mode auto
       [--session-id <uuid> | --resume <id> [--fork-session] [--resume-session-at <msg-uuid>]]
```

| Capability | Result |
|---|---|
| Long-lived process, multi-turn over stdin | ✅ `{"type":"user","message":{...}}` per turn; each turn ends with `result` |
| Permission prompts to host | ✅ needs `--permission-prompt-tool stdio`; arrives as `control_request{subtype:"can_use_tool"}`; reply `control_response{behavior:"allow", updatedInput}` / `{behavior:"deny", message}`. Includes `permission_suggestions` (for "always allow"). |
| `--permission-mode auto` | ✅ CC decides; only asks host when its classifier wants a human — **[decided]** default mode |
| AskUserQuestion | ✅ enabled under stdio prompt tool; arrives as `can_use_tool` with `tool_name:"AskUserQuestion"`; answer by returning `updatedInput` with `answers: {question: label}` |
| Interrupt | ✅ `control_request{subtype:"interrupt"}` → turn ends `result/error_during_execution`; session continues |
| Fork at arbitrary message | ✅ `--resume <id> --fork-session --resume-session-at <assistant-msg-uuid>` → new session id, memory truncated at that message (undocumented flag — wrap behind driver, test on upgrade) |
| Partial streaming | ✅ `stream_event` deltas |
| Cost/usage | ✅ per-turn `result.total_cost_usd`, `usage`; `rate_limit_event` gives subscription window utilization |
| Transcript on disk | `~/.claude/projects/<cwd-slug>/<session>.jsonl`, entries linked by `uuid`/`parentUuid` — used for import & fork targets |

Gotchas: without `--permission-prompt-tool stdio`, `--permission-prompts host` silently auto-denies. User hooks/plugins from `~/.claude` apply (good: parity with terminal use; but they can inject output — tolerate unknown event kinds).

### Driver interface

```rust
trait AgentDriver {
    async fn start(&self, spec: SessionSpec) -> Result<Box<dyn AgentSession>>;   // new | resume | fork{at}
}
trait AgentSession {
    async fn send(&mut self, input: UserInput) -> Result<()>;
    async fn interrupt(&mut self) -> Result<()>;
    async fn answer(&mut self, req: RequestId, answer: PermissionAnswer) -> Result<()>;
    fn events(&mut self) -> impl Stream<Item = DriverEvent>;   // raw vendor JSON + ephemeral flag
    async fn shutdown(self) -> Result<()>;
}
```

**Process lifecycle:** a CC process per *active* session. Idle sessions (no turn for N minutes, no pending prompt) are shut down and transparently `--resume`d on the next message — keeps memory flat with dozens of sessions.

## 7. Sync protocol **[implemented]**

One WebSocket per client, JSON text frames (`crates/atelier-proto`). Three resumable primitives:

- **sub** `{filter: {stream}|{prefix}, from}` → `events` batches (history, then live), a `live` marker, and `eph`
  frames (token deltas: live only, never persisted). Reconnect = re-sub from last seq + 1.
- **watch** `{view, scope?}` → `snapshot` of a server-side view (a projection table), then `patch` (upsert/remove
  rows) as projections report changed rows. Views: `projects`, `sessions` (scope = project, or none for all).
  Re-reading the latest row per change makes duplicates/reordering harmless; a lagged watcher re-snapshots.
- **rpc** `{method, params}` → `result | error`. Commands never return state.

Why both subs and watches: lists (projects, sessions) want server-side summaries that are cheap to keep current;
transcripts want raw events folded on the client by a per-vendor adapter (`web/src/agents/`). Folds there are pure
and tested; adding Codex = adding `agents/codex.ts`.

Back-pressure: bounded outbox per connection; if a client can't keep up it is disconnected and resumes from its seq.

**Auth:** a token in `<home>/token` (0600), created by whichever of server/app starts first; sent in `hello`.
Server binds loopback by default. Remote access + pairing: AT-012.

## 8. Frontend

- **SolidJS** **[decided]**, Vite, TypeScript. Types generated from `atelier-proto` with ts-rs (`make gen`).
- Design tokens from the mockups (`--paper/--ink/--red/--green/--blue/--gold`, light + dark), fonts bundled locally (IM Fell English, Source Serif 4, Fragment Mono). Ornament via CSS/inline SVG only.
- Performance rules:
  - Virtualized transcript and lists from day one.
  - Streaming: deltas append to one text node; markdown re-parsed only for the in-flight message, batched per animation frame.
  - Syntax highlighting lazy and visible-only (likely in a worker).
  - Event folding batched per frame; never re-render a finished message.
  - Target: 60 fps scroll on a 2,000-message transcript; < 16 ms input latency; cold start < 500 ms.
- Agent-drawn pages (later): sandboxed iframe, CSP `default-src 'none'`, injected link script.

## 9. Milestones

Status 2026-11-07: M0 done; M1 core done (sessions in worktrees, live transcript with streaming, permission and
question cards, interrupt/stop/archive, resume across server restarts — verified against the real CLI). Remaining M1
and M2 work is ticketed in `.atelier/tickets/`.


| # | Milestone | Done when |
|---|---|---|
| M0 | Skeleton: workspace, store, proto, server, Tauri+Solid shell, tokens | app connects to `atelierd`, shows live event count |
| M1 | **Sessions**: CC driver, session per worktree, live transcript, permission/question inbox, interrupt, resume; import `~/.claude/projects` read-only | daily-driveable on desktop |
| M2 | **Remote**: launchd daemon, Tailscale bind, pairing, reconnect/resume; iOS build | parity with CC + remote-control, from phone |
| M3 | Codex driver (`codex app-server`) | |
| M4 | `.atelier/` files, `at://` addresses, link index, backlinks, tickets/notes, MCP `ask`/`handoff` | mockup steps 1–2 |
| M5 | Checkpoints per file-changing turn, forks (native via `--resume-session-at`), history view | step 6 |
| M6 | Since-you-left digest, handoff notes | step 5 |
| M7 | Pages, peek, drift marks, live overlay | steps 3–4 |
| M8 | Proposals (parts, accept-in-parts, revise, alternatives) | step 8 |
| M9 | `spawn`, subagent tree, budgets | step 7 |
| — | Own harness: context as a projection of the log; RLM tools | |

## 10. Open questions

- Client-side event cache (IndexedDB) for instant cold start on the phone — measure first (AT-015).
- When to introduce Atelier's own agent loop (context as a projection of the log) — after M3.
