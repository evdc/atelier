We're building Atelier, an "agentic development environment", a new kind of app for humans working together with agents to build software, beyond the chat interface.

# Design

Less "fully autonomous software factory" and more "product studio" for those who still want some level of human involvement in the craft, but acknowledge humans aren't going to be writing or reviewing every line any more.

The goal is to allow the human to operate at a high level: design, specs, architecture, review - while drilling down to the line-of-code / agent-message-trace level if desired.

See `atelier-mockups.html` for some mockups of the UI, including the visual style:
- A minimal, subtly stated, elegant, tasteful style, inspired by classic Medieval/Rennaisance engravings and woodcut art - eg the Flammarion engraving, or (https://en.wikipedia.org/wiki/Ophanim#/media/File:Ezekiel-Vision-Merkaba.jpg) - imagery of sun breaking through the clouds, illumination, etc- black&white (or light parchment background) & muted earth tone red/green/blue/gold for accent color if any. Prefer using modern CSS/HTML, over images and heavy JS.

(The mockups are inspiration, not an authoritative spec.)

# Implementation

- Use Tauri (v2.x) with a performant frontend framework (e.g. Solid, Svelte) or no framework (vanilla TS), no React.
- Start with using Claude Code / Codex, via subprocesses/IPC, rather than building our own agent loop, to reuse the subscriptions I have. But keep this behind an interface, so we can add our own harness/loop later, if desired.
- The app has a client-server model: the server manages the coding agent sessions and state, the clients attach to it. This allows e.g. running the server on a persistent VPS, and the desktop client attaching to it (via Tailscale). This also allows for a mobile client (Tauri iOS) to the same sessions on the same server.
- Event-driven architecture: every Event (user message, agent response, tool call, tool response, question / permission gate, etc) is logged to persistent storage (sqlite) for later. The state is a function of the log. See https://tardigrade.sh/notes/why-tardigrade for some notes / design philosophy around this.
    - This is intended to eventually enable tooling like RLM / Context Language Models (https://github.com/facebookresearch/context-language-models) where the model can use tools to manage its own context instead of the harness doing that.

---

Think from first principles, break tasks down, be ambitious — we’re trying to do better than the state of the art, or at least the current default standard in the field — and always prefer solutions that are simple, elegant, maintainable, and high-performance. 

UI jank / slow performance is absolutely unacceptable here. Prefer "going faster via doing less" (a simpler solution that does less unnecessary work and has less overhead) vs. complex specific bespoke performance tricks (which can overfit to a particular benchmark). If I request any feature that would cause bad performance, stutter, jank, etc., I want you push back on me, and either we'll find a more performant way to do that or scrap it.
