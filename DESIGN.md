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
  **[decided] Worktree pool.** Archiving a clean session detaches its worktree and keeps it (at most 3 per project)
  instead of deleting it; the next session claims one with `git checkout -b`, so `target/` and `node_modules/` stay
  warm. A fresh worktree gets `target`, `node_modules` and `.venv` copy-on-write cloned from the project checkout
  (best effort, needs APFS/reflink). Pool membership is derived from git (detached, clean, under the project's
  worktrees dir, no live session) — no extra state. We re-implemented rather than depend on treehouse: it's ~100
  lines and has to share the archive/safety logic.

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
Server binds loopback by default. With `ATELIER_LISTEN_TAILSCALE=1` it also listens on the tailnet address, where
a peer owned by the same Tailscale user (`tailscale whois`) needs no token (AT-012). Loopback always needs it.

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
| M4+ | Automations (§10): signals, approval, runner, editor, MCP catalog | dragging a ticket to Doing starts a session on it |
| M5 | Checkpoints per file-changing turn, forks (native via `--resume-session-at`), history view | step 6 |
| M6 | Since-you-left digest, handoff notes | step 5 |
| M7 | Pages, peek, drift marks, live overlay | steps 3–4 |
| M8 | Proposals (parts, accept-in-parts, revise, alternatives) | step 8 |
| M9 | `spawn`, subagent tree, budgets (§11; messaging and `ask` land earlier, AT-026–033) | step 7 |
| — | Own harness: context as a projection of the log; RLM tools | |

## 10. Automations **[proposed]**

"When *x* happens, do *y*": e.g. *when a ticket is dragged into Doing, start a session on it*. Humans and agents
can both create, read and change automations.

### 10.1 The idea in one paragraph

Atelier already has the two things automations need. **Triggers** are events in the log (or state changes derived
from it). **Actions** are RPC `Call`s, the same commands the UI sends. An automation is a small authored file that
joins the two: *on* this trigger, *if* this guard holds, *do* these calls. It runs with its own actor
(`automation:<id>`), and every event it causes carries `cause` back to the event that triggered it. Nothing new
holds state. Definitions are files, runs are events, and "what's active" is a projection.

Principles, following §1:

- **One vocabulary.** What a human can do in the UI, an automation can do as an action, and an agent can do as an
  MCP tool, because all three are the same `Call` enum. Adding a `Call` makes it available to all three. Nothing is
  automation-only.
- **Not a programming language.** One trigger, a guard made of AND-ed field tests, and a list of calls whose
  parameters can be templated. There are no loops, no `else`, and no scripting. When the logic gets interesting,
  the action is "start an agent with this prompt": the agent handles the complex cases.
- **Visible causality.** Every session, message or ticket move an automation makes can show why it happened:
  *started by "Work on Doing tickets" because you moved AT-021 to Doing at 14:02*.
- **Nothing runs that a person hasn't approved.** Agents can write automations, but only the user can arm them
  (see §10.4).

### 10.2 Definitions are files

Automations are authored, so per §2 they live in git: `.atelier/automations/<slug>.md`. They use the same format
as tickets: YAML front matter for the rule and a markdown body that serves as the prompt template. Agents edit
them like any other file. Humans edit them in a form that writes the same file (§10.6). They show up in PRs, so
reviewers see them like any other change.

```markdown
---
name: Work on tickets moved to Doing
on:
  change: ticket.status        # a signal (§10.3)
  to: doing
if:
  subject.assignee: agent      # AND of field tests; optional
do:
  - session.create:            # any Call; params are templates
      project: "{{project.id}}"
      title: "{{subject.id}}: {{subject.title}}"
      prompt: "{{body}}"       # the markdown below, rendered
  - ticket.update:
      ticket: "{{subject.id}}"
      session: "{{steps.0.id}}" # results of earlier steps
limit: 6/hour
---

Work on {{subject.id}}. The ticket is `{{subject.path}}`; read it and the files it lists, then follow CLAUDE.md.
When you're done, set its status to `review`.
```

Only the project's own checkout is live (`<project.path>/.atelier/automations/`). An agent that edits its
worktree's copy has made a proposal. It takes effect when merged, and only after approval.

**Templates** support `{{dotted.path}}` lookups only. A value that is a single placeholder keeps its type (number,
object). These names are in scope:

| name | what |
|---|---|
| `event` | the triggering event (`kind`, `actor`, `seq`, `body.*`) |
| `subject` | the view row the trigger concerns (the ticket or session row), read after the change |
| `project` | the project row |
| `steps.<i>` | the result of step *i* (e.g. the `{ id }` that `session.create` returns) |
| `body` | the file's markdown body, itself rendered as a template |

**Guards** (`if`) map paths to tests, and every test must pass:
- a literal: equality;
- `{in: [...]}`, `{not: x}`, `{glob: "..."}` and `{exists: bool}`.

There is no OR; write two automations instead. Templates and guards together come to about 150 lines of Rust with
no dependency.

### 10.3 Triggers: events and signals

| `on:` | fires when | examples |
|---|---|---|
| `event: <kind>` | an event of that kind is appended (body fields testable in `if`) | `permission.requested`, `turn.ended`, `user.message`, `cc.result` |
| `change: <view>.<field>` (+ `from`/`to`) | a projection reports that field of a row changing | `ticket.status → doing`, `session.status → waiting`, `session.archived → true` |
| `manual` (+ `inputs`) | someone runs it: a button, the ⌘K palette, or an agent's `automation.run` tool | "Triage the inbox", "Spin up a review agent on this branch" |

**Why signals exist.** Most of what people care about is a *transition*: a ticket entering a column, a session
starting to wait. No single event says that. A ticket's status comes from a file edit, and a session's status comes
from process and turn events. So projections, which already report `Change { view, key, scope }`, may also report
a **signal**: `{ name: "ticket.status", key, scope, from, to }` for the fields they choose to expose. A projection
computes signals in `apply`, which keeps them deterministic. Projections later in `views::projections()` receive the
signals of earlier ones. The automations projection runs last.

Concretely, dragging a ticket works like this:
1. The UI drag sends `ticket.update { status }`.
2. That call rewrites the ticket's front matter.
3. The watcher appends `file.changed`.
4. The tickets projection folds the file and emits the `ticket.status` signal.

An agent that edits the file directly produces the same signal, and so does a `git pull` that changes it. What
moved the ticket doesn't matter, only that it moved.

These never trigger anything:
- **Ephemera** (token deltas, status): they aren't in the log.
- **Raw vendor events** (`cc.*`): they're allowed in `event:` triggers but discouraged. Prefer a signal from a
  projection, which survives a change of vendor.

Later, external sources (GitHub webhooks, CI) can append their payloads verbatim as `ext.github.*` (rule 2).
Triggering on them then needs no new mechanism.

### 10.4 Approval: agents propose, people arm

An automation runs only if the user has approved **its exact contents**, as with `direnv allow`:

- `automation.approved { path, hash }` is appended only by a user client.
  - Saving from the UI's editor appends it automatically.
  - Nothing an agent does (a file write, or MCP) can append it.
- Any change from a source other than the UI disarms it until someone reviews it: an agent's edit, a `git pull`,
  a teammate's commit. The UI shows it in red ("changed — review to arm") with a diff against the approved version.
- Pausing is personal and short-lived, so it's an event (`automation.paused` / `automation.resumed`) rather than a
  file edit. A file edit would churn git.

This is what makes agent-written automations safe without a separate permission system. An agent can draft "when
CI fails on my branch, wake me up with the log", and the person sees exactly what will run before it ever does.
Sessions an automation starts still follow the normal permission mode, and their prompts still reach the inbox.

### 10.5 Engine

Matching and executing are split along the line the codebase already draws: pure work goes in a projection, IO goes
in a supervisor.

**`automations` projection** (pure, runs in the append transaction):
- Folds two kinds of input:
  - `file.changed` events under `.atelier/automations/`, parsed into `automations(id, path, def, hash,
    approved_hash, paused, error)`;
  - `automation.*` events.
- For every event and signal it checks the armed automations. In memory it indexes them by trigger kind or signal
  name, so an event that no automation names costs one hash lookup.
- A match that passes its guard and `limit` inserts a row in `runs(id, automation, trigger_seq, state)` with
  `state = pending`. The id is `<trigger_seq>:<automation>`.
- Matching is a function of the log, so it is exactly-once: a crash can't lose a trigger, and a rebuild can't invent
  one.
- Rate limits count earlier `runs` rows by their trigger events' `ts`, never the clock.

**Runner** (IO, inside `atelier-server`, alongside the supervisor):
- Watches `runs` for rows with `state = pending`.
- For each step it does three things in order:
  1. appends `automation.step.started { run, i, call }` (the call with its templates rendered);
  2. runs the call through the same `rpc::handle`, with `Actor::Automation(id)` and `cause = trigger_seq`;
  3. appends `automation.step.finished { run, i, result | error }`.
- An error stops the run. The run's last event is `automation.finished { run, ok }`.
- Required refactor: `rpc::handle` takes `(actor, cause)` instead of hard-coding `Actor::User`. Agents' MCP calls
  then use the same path with `Actor::Agent`.

**Delivery is at-most-once.** On restart, a step that has `started` but no `finished` is marked `interrupted` and
the run fails visibly, in red. It is not retried. Retrying an action like starting an agent is worse than asking.

After a rebuild (a projection version bump), pending runs whose trigger is older than the rebuild point are
appended as `automation.skipped`. Changing the fold logic must never replay last month's triggers.

**Loops.** Automations may trigger other automations; that chaining is what makes them composable. Two limits
apply, both computed from the `cause` chain:
- A run whose cause chain already contains the same automation does not start.
- A chain stops at depth 8.

The per-automation `limit` (default `20/hour`) bounds the rest. A tripped limit appends
`automation.limited { automation, trigger_seq }` once per window. It shows in red; it is not dropped silently.

New event kinds (stream `p/<project>`; run events on `p/<project>/a/<automation>`): `file.changed` (from the
watcher, M4), `automation.approved`, `.paused`, `.resumed`, `.step.started`, `.step.finished`, `.finished`,
`.skipped`, `.limited`. New calls: `automation.approve`, `.pause`, `.resume`, `.run { automation, inputs }`,
`.test`, `.save { path, content }` (a user-only file write that approves).

### 10.6 How humans see it

The **Automations** view is per project: one row per automation, read as a sentence.

```
 Automations — atelier                                               + New
 ─────────────────────────────────────────────────────────────────────────
 ● Work on tickets moved to Doing                          14 runs · 2h ago
   when a ticket's status becomes doing, if assignee is agent
   → start a session · update the ticket
 ● Review when a session goes idle with changes             3 runs · today
 ○ Nightly dependency check                     paused · by you, yesterday
 ● Allow `npm test` in automation-started sessions   changed by agent:7f3a
   review to arm  ·  diff                                     ← red: needs you
```

- Dot colours follow the tokens:
  - green: armed and healthy;
  - blue: a run in flight;
  - red: needs review, failing, or limited;
  - hollow: paused.
- The **editor** is a form over the file, with the same three parts in the same order: *When* (trigger picker built
  from the catalog, §10.7), *If* (field tests), *Do* (a list of calls with templated fields; for `session.create`
  the prompt is the markdown body). "Edit as text" shows the raw file, and the two stay in sync because the file is
  the model.
- **Test** runs the matcher over this project's last 7 days of log and lists what *would* have fired, with every
  call rendered, without running anything (`automation.test`). It's the cheapest way to trust a rule, for a person
  or an agent.
- **Run history** comes from `automation.*` events. Each run links to its trigger event and to the events it
  caused: the session it started, the ticket it moved.
- **Creating in context** is where most automations should come from:
  - a board column header's menu: "When a ticket enters *Doing*…" opens the editor with the trigger filled in;
  - any event or card in a transcript: "Automate this…" (e.g. on a permission card, "always answer like this when
    …").
- **Provenance** shows on everything an automation touched: one quiet line in gold under the session header,
  *started by "Work on tickets moved to Doing" · AT-021 → Doing by you*.

### 10.7 How agents see it

Agents use the planned Atelier MCP server (§4, M4). It exposes the same surface:

| tool | what |
|---|---|
| `atelier.describe` | the catalog: every event kind, signal and `Call`, with fields and doc comments, generated from the Rust types. The UI's trigger and action pickers read the same catalog. |
| `events.query { stream?, kinds?, since? }` / `views.read { view, scope? }` | read the log and views. Results are bounded and paginated, never the whole log. |
| one tool per `Call` | e.g. `session.create`, `ticket.update`. Each `Call` declares whether agents may use it; `automation.approve`, `.save`, `.resume` and `project.remove` are user-only. |
| `automation.test { path \| content }` | dry-run, as in the UI |
| `automation.run { automation, inputs }` | run a `manual` automation (it still has to be armed) |

Agents need no `automation.create`. Writing `.atelier/automations/x.md` is how they create one, and the result
lands in the person's "review to arm" list. Agents already know how to edit files, the change shows in the
session's diff, and nothing is hidden in the database.

The catalog is generated, not written by hand. Doc comments on `domain.rs` bodies and `Call` variants are the
documentation, extracted at build time like the ts-rs bindings (`make gen`). See the open question in §11 on the
schema tool.

### 10.8 Examples it should handle

| when | if | do |
|---|---|---|
| `ticket.status → doing` | assignee is agent | `session.create` with the ticket as prompt; `ticket.update { session }` |
| `session.status → idle` | `subject.origin.automation` is the one above | `session.send` "Run `make check`; if green, set the ticket to review" |
| `session.archived → true` | session has a ticket | `ticket.update { status: done }` |
| `ticket.status → review` | — | `session.create` "Review the diff on `{{subject.branch}}` against the ticket's acceptance list" |
| `permission.requested` | `event.body.tool: Bash`, input matches `{glob: "npm test*"}`, session started by an automation | `session.answer allow` (an auto-approver. It must be armed like any other, and its provenance shows on the answered card.) |
| `manual` with input `branch` | — | start three sessions with different approaches to the same prompt (fan-out, before M9's `spawn`) |

### 10.9 Not yet

- **Time.** Schedules ("nightly") and delays ("waiting for more than 10 min") need a clock, and projections must
  not read one. The plan is a timer task that appends `timer.fired { automation, at }` when due. The trigger is
  then an ordinary event and the log stays complete.
  - A delay is a run that waits for its `timer.fired`, then re-checks its guard.
  - Build it after the event and signal triggers have proven themselves.
- **Waiting on a later event.** Multi-step workflows ("start an agent; when it's done, run tests; if they fail,
  tell it") are written today as chained automations keyed on provenance (row 2 above). An `await` step that
  parks a run until a matching signal arrives would make them one file. Add it only if chaining proves clumsy.
- **Branching or expressions** beyond guards. When a rule needs them, start an agent with a prompt instead.

### 10.10 Sequencing

Depends on M4: tickets as files, the `.atelier/` watcher (`file.changed`), the tickets projection and board, and
the MCP server. Suggested tickets, in order:
1. Signals from projections: `ticket.status`, `session.status`, `session.archived`, plus the writer change that
   passes signals to later projections.
2. `rpc::handle(actor, cause)`, and `Actor::Automation`.
3. The automations projection: parse, approve or pause, match, the `runs` table, limits and loop guard. Test it the
   way `views/*` are tested: append events, assert rows.
4. The runner, including at-most-once handling and the rebuild skip. Add an e2e test against the fake CLI: move a
   ticket, a session starts, its `cause` chain reaches the move.
5. The catalog and `automation.test`.
6. The UI: the list, the form editor, the run history, in-context creation and provenance lines.
7. MCP exposure (with M4's MCP server).

## 11. Sessions talking to sessions **[proposed]**

Agents can find other sessions, read them, message them, ask them questions, and start new ones. These are tools
on the Atelier MCP server (§4), so every exchange is in the log, shows in both transcripts, and works the same for
any vendor. Tickets AT-026 to AT-033.

### 11.1 Why: persistent context is what we already have

The argument, after *On the nature of the swarm* (hallerite.com, 2026), goes like this:
- One context window caps what one agent can do. Compaction extends it in time but loses things, because it has to
  guess what will matter later.
- A one-off subagent (Claude Code's `Task`) summarizes *after* the question is known, which is better, but then it
  forgets everything. You can't ask it a follow-up.
- A **persistent** agent avoids both problems. You never decide in advance what to keep, because you can go back and
  ask it.

Atelier already keeps every session around: its log, its worktree, and its resumable CC session. What's missing is a
way for one agent to reach another. So most of this section is plumbing, not new concepts. Ideas taken from the
article:

| idea | here |
|---|---|
| Persistent beats one-off when follow-ups are likely | Spawned children are ordinary sessions. Any session can `ask` them later, including archived ones (§11.4). |
| **Mesh** for communication, **tree** only where it pays for itself | Messaging is a mesh: any session can reach any other. Spawning makes a tree, and the tree is used only for accounting (budgets, limits, the Agents view). Replies don't have to route through the root. |
| Roles emerge: the agent that built context on X becomes the one to ask about X | Discovery is by search over what sessions *said* (AT-024/025), not by assigned roles. Agents are told to look for an existing session that knows before they spawn a new one or re-derive. |
| Communication is lossy and costs tokens | Messages carry references (`seq`s, later `at://` addresses) to the source, so the recipient can `read` the original instead of trusting a summary. |
| Shared structured state as the source of truth | That's the log plus `.atelier/` (tickets, later a channel). Agents coordinate through tickets as much as through messages. |
| Let agents change their own organization | Already §10: agents write automations and people arm them. |
| Failure modes: duplicated work, endless chatter, context rot | Search before spawning; hop limits and deadlock checks (§11.3); asking a *fork* so the expert's context isn't spent on your question (§11.4). |

### 11.2 Tools

All tools are on the `atelier` MCP server, which atelierd serves on loopback at `/mcp` (AT-026). Each agent process
gets a per-process bearer token, so the server knows which session is calling, and calls run as
`Actor::Agent(session)`.

| tool | what | ticket |
|---|---|---|
| `sessions { project?, status?, include_archived? }` | Directory: one line per session (id, title, status, branch, parent, last activity). Your own row is marked. | AT-026 |
| `read { session, around? \| before?, max_chars? }` | A bounded text rendering of a session's transcript, made of the same documents the search index uses. | AT-026 |
| `search { q, project? }` | `search.query` (AT-024/025) as a tool. | AT-027 |
| `send { to, text, refs? }` | Asynchronous. Delivered as a new turn in the target, waking it if idle. Replies come back the same way. | AT-028 |
| `spawn { task, title?, workspace?, model?, budget?, wait?, review? }` | Starts a child session. `workspace` can branch from or share any session (§11.5). Returns `{ id }`, or with `wait`, the child's answer. | AT-029 |
| `ask { to, question, fork? }` | Synchronous. Blocks until the target's turn ends and returns its final text. With `fork`, a throwaway fork answers. | AT-030, AT-031 |

Read tools and `send`/`ask` are pre-allowed (`--allowedTools mcp__atelier__…`). `spawn` goes through the normal
permission mode, because it creates a worktree and spends money. "Always allow" (AT-007) covers people who want it
unattended. The server enforces the structural limits itself (§11.3), so pre-allowing the rest is safe.

These are the `Call`s of §10.7, so they are also UI actions and automation steps. `session.send` with an agent actor
appends `agent.message` instead of `user.message`.

### 11.3 Messages

- **Event.** `agent.message { from, text, mode: tell|ask, hops, refs[], ask_id? }` goes in the *target's* stream with
  `actor = agent:<from>` and `cause` = the sender's `cc.assistant` event that made the tool call. The sender's side is
  already in its own transcript as the tool call and its result, so nothing is duplicated.
- **Delivery.** The supervisor delivers it like a user message. If the session isn't running, delivery resumes it. If
  a turn is in progress, the message queues (CC queues stdin turns mid-turn; see `queued_turn_count`). It never
  interrupts the target. Delivering to an archived session is refused, and the error suggests `ask { fork: true }`.
- **Envelope.** The agent sees:
  ```
  <atelier-message from="7f3a" title="Fix WAL checkpoint" project="atelier" mode="tell">
  …text…
  </atelier-message>
  (From another session, not the person. To reply, call send with to="7f3a". Your reply text in this turn is not delivered.)
  ```
- **Hops.**
  - A message's `hops` is one more than the hops of the input that started the sender's current turn. A person's
    message counts as 0.
  - At 6 the send is refused with "hop limit: ask the person". Any human message resets the count.
  - This stops ping-pong without a clock. The sessions projection keeps the last input's hops.
- **Deadlock.** `ask` blocks the asker's turn. Before blocking, the server walks the wait-for graph of open asks; if
  the new ask would close a cycle (A asks B, B asks A), it fails at once.
- **UI.** In the target's transcript an agent message is a card with a gold rule, *from "Fix WAL checkpoint"*, which
  links to the sender. It is not a red "needs you" item and doesn't notify.

### 11.4 Asking a fork

Asking a persistent agent costs it context: the question and answer stay in its window and add to its rot. CC can
fork a session (`--resume <id> --fork-session`), so `ask { fork: true }` answers from a throwaway copy:
- The fork has the target's full memory up to now.
- It runs as a read-only share of the target's workspace (§11.5), answers once, and is archived.
- The target is never disturbed. Its context isn't spent, it can be mid-turn, and it can even be archived.

This makes every finished session a standing oracle on whatever it worked on, and it's something only an
event-sourced host that keeps every session can offer. Forked answers are recorded like any `ask`. The fork is a
hidden child session (`parent = asker`, `origin = fork-of:<target>`) whose cost lands in the asker's tree.

### 11.5 Workspaces from other sessions (AT-033)

A session is two independent choices: a **context** (fresh, or a fork of a session) and a **workspace**. Besides a
new worktree or the project directory, a workspace can now come from another session X:

- **Share X, read-only** (`WorkspaceMode::Shared`, `from: X`).
  - It runs in X's directory and sees X's live, uncommitted edits.
  - Starting it with Edit/Write/NotebookEdit disallowed plus a system note keeps it read-only. That's a convention,
    not a sandbox: Bash can still write, and any write shows in X's Changes.
- **Branch from X.** A new worktree at a *snapshot* of X's working tree, uncommitted files included.
  - The snapshot is taken with a temporary index (`read-tree HEAD; add -A; write-tree; commit-tree`) and kept on
    `refs/atelier/snap/<id>`, so X is never touched.
  - Its `base` is the snapshot, so the new session's Changes show only its own work.
  - M5's per-turn checkpoints reuse the same helper.

| context | workspace | is |
|---|---|---|
| fresh | share X | a reviewer: unbiased, sees the real diff, re-checks later with memory of what it flagged |
| fresh | branch from X | a parallel subtask or an alternative from where X is (`spawn`'s default) |
| fork of X | share X | ask-a-fork (§11.4) |
| fork of X at m | branch from X's checkpoint at m | M5's "fork here" |

**One owner per worktree.** Only the session that created a worktree writes to it and releases it.
- Archiving the owner while someone is still sharing keeps the worktree.
- The last sharer's archive runs the owner's usual release.
- The pool already skips any path a live session uses.

Write-sharing (pairing) would need a turn-level lock, so it waits; branch-from covers parallel writes.

The person gets this as "Review changes" on a session, which starts a fresh session sharing it read-only with a review
prompt, and as two new options in the new-session form. Agents get it through `spawn { workspace }` and
`spawn { review: "self" }`.

### 11.6 Spawn and trees

This is mockup step 7, M9.
- `SessionCreated` gains `parent: Option<SessionId>` and `budget` (both `#[serde(default)]`).
- Child workspaces (§11.5):
  - `{ branch: self|id }`, the default, branches from a snapshot of the parent, including uncommitted work;
  - `{ share: self|id }` is read-only, for reviewers and researchers;
  - `fresh` is a new worktree from the project's HEAD;
  - `project` is the project checkout.
- The child's first message is the task, wrapped in an envelope that says who spawned it and how to report back.
  With `wait`, its first turn's final text is the tool result, as with `ask`.
- Limits, checked in `rpc`: depth ≤ 3, ≤ 4 live children per session, and the tree's budget (AT-032). They're
  configurable in Settings and every refusal says why.
- The tree is used for accounting and the Agents view. It doesn't restrict who can talk to whom.
- The sidebar nests children under their parent. The session header shows provenance in gold: *spawned by "Move
  sessions to opaque tokens"*.

### 11.7 Not yet

- **A project channel.** One shared stream that agents post to and read, for broadcast instead of N² messages ("I'm
  changing the writer; don't touch `store/`"). Build it when a real swarm shows the need.
- **"Knows about" lines.** An agent-written one-line summary per session, refreshed every N turns like the autotitle
  (AT-017), shown in `sessions` so routing questions to specialists gets cheap. Search may be enough.
- **Interrupting another session** ("urgent"). People can, agents can't, for now.
- **Observed subagents.** Drawing CC's own `Task` subagents (`parent_tool_use_id`) in the same tree, read-only.
- **Returning proposals** (`returns: "proposal"` in the mockup). Waits for M8.

## 12. Open questions

- Client-side event cache (IndexedDB) for instant cold start on the phone — measure first (AT-015).
- When to introduce Atelier's own agent loop (context as a projection of the log) — after M3.
- Automations (§10):
  - Catalog schemas: should we use `schemars` (one dependency, JSON Schema the MCP tools need anyway), or a
    hand-rolled walk over ts-rs output? Leaning towards `schemars`.
  - Is the board's set of columns per project (`.atelier/board.toml`) or simply the distinct ticket statuses?
  - Name: "automations" is the plain word. Keep "workflow" for the multi-step `await` form, if we ever build it.
- Sessions talking to sessions (§11):
  - CC ships its own cross-session `SendMessage`/`ListAgents` tools. Disable them in Atelier sessions
    (`--disallowedTools`) so there is one mechanism, and it's logged? Leaning yes; verify in AT-026's spike.
  - Should `spawn` be pre-allowed instead of going through the permission mode? Start conservative.
