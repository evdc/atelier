# Atelier — instructions for agents

Atelier is an agentic development environment: a server (`atelierd`) that runs coding agents (Claude Code today) in
git worktrees and logs everything they do as events, plus thin clients (desktop via Tauri, phone later) that attach
to it. Read `DESIGN.md` for the why; this file is the how.

## Map

```
crates/atelier-core     Event envelope, ids, stream names, typed event bodies (domain.rs). Pure, no IO.
crates/atelier-store    SQLite event log: append (group commit), projections, commit feed. Generic.
crates/atelier-proto    Wire protocol + view row types. TS bindings generated from here into web/src/gen.
crates/atelier-driver   AgentDriver trait + Claude Code driver (stream-json over stdio).
crates/atelier-server   atelierd: views/ (projections+queries), supervisor (agent processes), rpc, sync (WebSocket).
app/                    Tauri v2 desktop host. Starts atelierd if needed, injects {url, token}, runs it as a launchd agent when bundled (server.rs). Few Tauri commands.
web/                    SolidJS UI. lib/ (conn, views, router), agents/ (transcript folds), ui/ (components).
.atelier/tickets/       Work items. Each is self-contained; pick one, follow it, tick its acceptance list.
spikes/                 Throwaway experiments that informed decisions (e.g. the CC stream-json protocol).
```

Data flow: `rpc → append events → projections (same tx) → commit feed → subs (raw events) / watches (view rows) → UI`.
State is never sent by RPC results; clients converge by folding the same log.

## Rules

1. **The log is the truth.** Anything that happens is an event. Never mutate state that isn't a projection of the log.
   Event bodies in `domain.rs` may only gain optional (`#[serde(default)]`) fields — the log is immutable. Anything
   else is a new kind.
2. **Store vendor output verbatim** (`cc.*` kinds), interpret it in projections/folds. Don't normalize at ingest.
3. **Projections are deterministic** (no clock, no IO beyond their own tables) and keep all state in their tables.
   Changing fold logic or schema ⇒ bump `version()`; it rebuilds from the log.
4. **Performance is a feature.** No jank, ever. Prefer doing less over clever tricks:
   - don't persist ephemera (token deltas), don't ship clients what they didn't ask for, don't re-render finished items;
   - batch per animation frame; virtualize long lists; parse/highlight lazily and only what's visible;
   - never add a dependency for something 30 lines can do. The JS bundle is ~17 KB gzipped — keep it small.
   If a feature can't be done without jank, say so in your hand-back instead of shipping it.
5. **Design language**: parchment and ink, after woodcut engravings. Only CSS variables from `web/src/styles/tokens.css`
   carry colour — red = needs you, blue = running/live, green = done, gold = the agent / things to notice. Hairlines,
   not shadows; the display face (IM Fell) for headings only. Restraint over decoration. Light and dark both work.
6. **Protocol changes**: edit Rust types, run `make gen`, commit the regenerated `web/src/gen/*`. Never hand-edit gen.
7. Keep comments for *why*; names and types should say *what*.

## Commands

```
make dev     server (home ~/.atelier-dev) + UI at http://localhost:5173
make app     desktop app (uses ~/.atelier)
make test    cargo test --workspace && vitest
make check   fmt + clippy -D warnings + bindings fresh + tsc   ← must pass before you hand work back
make gen     regenerate TS bindings
```

Tests: Rust unit tests next to code; server end-to-end over a real WebSocket in `crates/atelier-server/tests/e2e.rs`,
using `crates/atelier-driver/tests/fixtures/fake-claude.sh` instead of the real CLI. Web folds are pure and tested
with vitest (`*.test.ts`). Extend the fake CLI when you need new agent behaviour in tests.

## Working on a ticket

1. Read the ticket, then the files it lists. Don't widen scope; note follow-ups in the ticket instead.
2. Make it work, with tests where the ticket says. Run `make check` and `make test`.
3. Set the ticket's `status: done` and tick its acceptance items. Summarize what changed and anything you
   couldn't do, honestly.
