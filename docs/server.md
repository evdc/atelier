# Running atelierd

## On a VPS (one command)

On a Linux box that already has Tailscale (`tailscale up` as yourself, not with a tagged key):

```
curl -fsSL https://raw.githubusercontent.com/evdc/atelier/main/scripts/install-server.sh | sudo sh
```

It installs the release binary for your CPU, creates an `atelier` user, installs Claude Code for it, writes a systemd
unit that listens on loopback and the tailnet address only, and prints what is left to do. Re-run it to upgrade. Set
`ANTHROPIC_API_KEY=…` to skip the interactive Claude login, or `--bin ./atelierd` to install a binary you built.
Releases come from `.github/workflows/release.yml` (push a `v*` tag).

On each Mac, put the server's address in `~/.atelier/remote.json` (`{"url": "100.x.y.z"}`, plus `"token"` for a tagged
or shared device) and relaunch. The app then only connects: it starts no local server. Delete the file to go back.


## macOS

Atelier.app bundles `atelierd` and, on launch, installs `~/Library/LaunchAgents/sh.atelier.server.plist`
(`RunAtLoad`, `KeepAlive`, log in `~/.atelier/atelierd.log`) and loads it with `launchctl bootstrap gui/$UID`. The
server therefore starts at login and keeps running when the app quits. After an app upgrade the plist is rewritten if
the app moved, and the server is restarted if it is older than the app. Footer → connection → *Server* shows its
status and has a restart button.

Server settings go in `~/.atelier/env` (`KEY=VALUE` lines, `ATELIER_*` only), because an app started from Finder has no
shell environment. To reach the server from your phone over Tailscale:

```
echo ATELIER_LISTEN_TAILSCALE=1 >> ~/.atelier/env
```

then relaunch Atelier (it rewrites the plist and reloads the server when the file changes).

If launchd can't be used the app falls back to starting the server itself (it then survives quitting the app but not a
restart or logout) and says so in that panel. To remove the login item:
`launchctl bootout gui/$UID/sh.atelier.server && rm ~/Library/LaunchAgents/sh.atelier.server.plist`.

## Linux (e.g. a VPS)

Install `atelierd` somewhere on disk, then `~/.config/systemd/user/atelierd.service`:

```ini
[Unit]
Description=Atelier server

[Service]
ExecStart=%h/.local/bin/atelierd
Restart=always
# Agents (claude, git) must be on PATH.
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
# Reachable from your tailnet; see DESIGN.md.
# Environment=ATELIER_LISTEN_TAILSCALE=1

[Install]
WantedBy=default.target
```

```
systemctl --user daemon-reload
systemctl --user enable --now atelierd
loginctl enable-linger $USER     # keep it running when you're logged out
journalctl --user -u atelierd -f
```

The token for remote clients is in `~/.atelier/token`.
