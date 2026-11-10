#!/bin/sh
# `make app`: the desktop app in development, over your real data (~/.atelier, port 7747), for dogfooding.
#
# The installed app's server (launchd agent sh.atelier.server) owns that home and port, and two servers must never
# share a log, so it's stopped for the session and put back on exit. The freshly built server runs in the
# foreground of this terminal instead, so its logs are here.
set -u

LABEL=sh.atelier.server
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
HOME_REAL="${ATELIER_HOME:-$HOME/.atelier}"
PORT=7747

restore=0
if launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; then
  echo "app-real: pausing the installed server ($LABEL); it comes back when you quit"
  launchctl bootout "$DOMAIN/$LABEL" || exit 1
  restore=1
fi
# A server left over from a previous run (the app starts one if none is listening).
lsof -ti tcp:$PORT -sTCP:LISTEN | xargs kill 2>/dev/null

server=
cleanup() {
  trap - EXIT INT TERM
  [ -n "$server" ] && kill "$server" 2>/dev/null && wait "$server" 2>/dev/null
  if [ "$restore" = 1 ]; then
    echo "app-real: restarting the installed server"
    launchctl bootstrap "$DOMAIN" "$PLIST"
  fi
}
trap cleanup EXIT INT TERM

# Server settings the launchd agent gets from <home>/env (e.g. ATELIER_LISTEN_TAILSCALE=1 for the phone).
if [ -f "$HOME_REAL/env" ]; then
  set -a
  # shellcheck disable=SC1090
  . /dev/stdin <<EOF
$(grep -E '^[[:space:]]*ATELIER_[A-Z_]+[[:space:]]*=' "$HOME_REAL/env" | grep -vE '^[[:space:]]*ATELIER_(URL|TOKEN|SERVER_BIN)[[:space:]]*=' | sed 's/[[:space:]]*=[[:space:]]*/=/')
EOF
  set +a
fi

export ATELIER_HOME="$HOME_REAL" ATELIER_LISTEN="127.0.0.1:$PORT"
./target/debug/atelierd &
server=$!

(cd app && ATELIER_URL="ws://127.0.0.1:$PORT/ws" cargo tauri dev)
