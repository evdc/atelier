#!/bin/sh
# Install atelierd as a systemd service on a Linux VPS that already has Tailscale, and print how to connect to it.
#
#   curl -fsSL https://raw.githubusercontent.com/evdc/atelier/main/scripts/install-server.sh | sudo sh
#   sudo sh install-server.sh --bin ./atelierd        # a binary you built or copied over yourself
#
# Safe to run again: it upgrades the binary and restarts the service, keeping the data in ~atelier/.atelier.
#
# Environment:
#   ATELIER_USER        user the server and its agents run as (default: atelier; created if missing)
#   ATELIER_VERSION     release tag to install (default: latest)
#   ATELIER_REPO        GitHub repo to download from (default: evdc/atelier)
#   ANTHROPIC_API_KEY   if set, written to /etc/atelierd.env so agents can authenticate without `claude login`
set -eu

REPO="${ATELIER_REPO:-evdc/atelier}"
USER_NAME="${ATELIER_USER:-atelier}"
VERSION="${ATELIER_VERSION:-latest}"
BIN_SRC=""

say() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN_SRC="${2:-}"; shift 2 ;;
    -h|--help) sed -n '2,13p' "$0" 2>/dev/null; exit 0 ;;
    *) die "unknown argument $1" ;;
  esac
done

[ "$(uname -s)" = Linux ] || die "this installs a systemd service, so it only runs on Linux"
[ "$(id -u)" = 0 ] || die "run as root (sudo sh install-server.sh)"
command -v systemctl >/dev/null || die "systemd is required"
command -v git >/dev/null || die "git is required (apt-get install -y git)"
command -v tailscale >/dev/null || die "tailscale isn't installed; see https://tailscale.com/download"
TS_IP="$(tailscale ip -4 2>/dev/null | head -n 1 || true)"
[ -n "$TS_IP" ] || die "tailscale isn't connected; run: tailscale up"

# --- the binary ---------------------------------------------------------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if [ -z "$BIN_SRC" ]; then
  case "$(uname -m)" in
    x86_64) target=x86_64-unknown-linux-gnu ;;
    aarch64|arm64) target=aarch64-unknown-linux-gnu ;;
    *) die "no prebuilt server for $(uname -m); build it (cargo build --release -p atelier-server) and pass --bin" ;;
  esac
  if [ "$VERSION" = latest ]; then
    url="https://github.com/$REPO/releases/latest/download/atelierd-$target.tar.gz"
  else
    url="https://github.com/$REPO/releases/download/$VERSION/atelierd-$target.tar.gz"
  fi
  say "downloading $url"
  curl -fsSL "$url" -o "$tmp/atelierd.tar.gz" ||
    die "download failed (is the repo private, or the release missing?). Build it and use --bin instead."
  tar -xzf "$tmp/atelierd.tar.gz" -C "$tmp" atelierd
  BIN_SRC="$tmp/atelierd"
fi
[ -f "$BIN_SRC" ] || die "$BIN_SRC doesn't exist"
install -m 0755 "$BIN_SRC" /usr/local/bin/atelierd

# --- the user, and the tools agents need ---------------------------------------------------------------------------
if ! id "$USER_NAME" >/dev/null 2>&1; then
  say "creating user $USER_NAME"
  useradd --create-home --shell /bin/bash "$USER_NAME"
fi
HOME_DIR="$(getent passwd "$USER_NAME" | cut -d: -f6)"
as_user() { runuser -u "$USER_NAME" -- env HOME="$HOME_DIR" PATH="$HOME_DIR/.local/bin:/usr/local/bin:/usr/bin:/bin" "$@"; }

# The server asks the tailscale CLI for its address and who a peer belongs to; let the service user do that.
tailscale set --operator="$USER_NAME" >/dev/null 2>&1 || say "couldn't set the tailscale operator; if the server can't read tailnet status, run: tailscale set --operator=$USER_NAME"

if ! as_user sh -c 'command -v claude' >/dev/null 2>&1; then
  say "installing Claude Code for $USER_NAME"
  as_user sh -c 'curl -fsSL https://claude.ai/install.sh | bash' || say "Claude Code install failed; install it for $USER_NAME yourself"
fi

# --- the service ---------------------------------------------------------------------------------------------------
# Secrets live here, not in the unit, so `systemctl cat` doesn't print them.
touch /etc/atelierd.env
chmod 600 /etc/atelierd.env
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  grep -v '^ANTHROPIC_API_KEY=' /etc/atelierd.env > "$tmp/env" || true
  printf 'ANTHROPIC_API_KEY=%s\n' "$ANTHROPIC_API_KEY" >> "$tmp/env"
  cat "$tmp/env" > /etc/atelierd.env
fi

cat > /etc/systemd/system/atelierd.service <<UNIT
[Unit]
Description=Atelier server
After=network-online.target tailscaled.service
Wants=network-online.target

[Service]
User=$USER_NAME
ExecStart=/usr/local/bin/atelierd
Restart=always
RestartSec=2
# Agents need claude and git on PATH.
Environment=PATH=$HOME_DIR/.local/bin:/usr/local/bin:/usr/bin:/bin
# Loopback always, plus the tailnet address. Never a public interface.
Environment=ATELIER_LISTEN_TAILSCALE=1
EnvironmentFile=-/etc/atelierd.env

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable atelierd >/dev/null 2>&1
systemctl restart atelierd

say "waiting for the server"
up=""
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
  if curl -fsS "http://$TS_IP:7747/health" >/dev/null 2>&1; then up=1; break; fi
  sleep 1
done
[ -n "$up" ] || { journalctl -u atelierd -n 20 --no-pager >&2; die "the server didn't come up on $TS_IP:7747 (log above)"; }

TOKEN="$(cat "$HOME_DIR/.atelier/token" 2>/dev/null || true)"
# The tailnet IP always resolves; a MagicDNS name only does if the client has MagicDNS on.
HOST="$TS_IP"

# --- what's left, and how to connect -------------------------------------------------------------------------------
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  claude_step="  - Claude Code will use the ANTHROPIC_API_KEY you passed (stored in /etc/atelierd.env)."
else
  claude_step="  - Sign Claude Code in:   sudo -iu $USER_NAME claude        (or re-run with ANTHROPIC_API_KEY=... set)"
fi

cat <<EOF

atelierd is running on $TS_IP:7747 (tailnet only), as user $USER_NAME.

Still to do on this machine, once:
$claude_step
  - Put your repositories somewhere $USER_NAME can read, e.g.
      sudo -iu $USER_NAME git clone <url> ~/Projects/<name>
    (for pushing, give that user a deploy key: sudo -iu $USER_NAME ssh-keygen -t ed25519)

On your laptop and desktop (macOS), then relaunch Atelier:

  mkdir -p ~/.atelier && echo '{"url": "$HOST"}' > ~/.atelier/remote.json

That makes the app a pure client: it starts no local server. Delete the file to go back to a local one.
Your own tailnet devices need no token. If a device is tagged or shared, add one:

  {"url": "$HOST", "token": "$TOKEN"}

EOF
