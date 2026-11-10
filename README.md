# Atelier

An agentic development environment. More "collaborative workshop for humans and agents", less "fully autonomous software factory".

![alt text](<docs/Screenshot 2026-11-10 at 3.04.54 PM.png>)

This README is intended for humans. See [DESIGN.md](DESIGN.md) for the "why" and guiding principles. Agents, see `CLAUDE.md` / `AGENTS.md`.

## Architecture

a server (`atelierd`) that runs coding agents in git worktrees and logs everything they do, plus thin clients (desktop, iPhone) that attach to it.  `make help` lists the dev tasks.

See [docs/mobile.md](docs/mobile.md) for a guide for building the phone app and connecting it to the laptop over Tailscale.