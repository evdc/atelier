#!/bin/sh
# Push the working tree to your VPS, build atelierd there and restart the service: the dev loop for server changes.
# Run from the repo root on your Mac, after scripts/install-server.sh has set the box up once.
#
#   ATELIER_DEPLOY_HOST=root@my-vps ./scripts/deploy-server.sh
#   ./scripts/deploy-server.sh --logs        # follow the service log after restarting
#
# Builds on the VPS (rsync + an incremental release build) because that needs no cross toolchain on the Mac; the first
# build is slow, later ones only recompile what changed. Uncommitted changes are included, ignored files aren't.
#
# Environment:
#   ATELIER_DEPLOY_HOST       ssh target, e.g. user@100.x.y.z or a Host from ~/.ssh/config (required)
#   ATELIER_DEPLOY_SSH_OPTS   extra ssh options, e.g. "-i ~/.ssh/hetzner -p 2222"
#   ATELIER_DEPLOY_SRC        where the source lands on the VPS (default: .cache/atelier-src, relative to ~)
#   ATELIER_DEPLOY_SERVICE    systemd unit to restart (default: atelierd)
# The ssh user needs passwordless sudo (or be root) to replace /usr/local/bin/atelierd and restart the unit, and a Rust
# toolchain (curl https://sh.rustup.rs | sh) plus a C compiler (apt-get install -y build-essential).
set -eu

say() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

LOGS=""
for arg in "$@"; do
  case "$arg" in
    --logs) LOGS=1 ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    *) die "unknown argument $arg" ;;
  esac
done

HOST="${ATELIER_DEPLOY_HOST:-}"
[ -n "$HOST" ] || die "set ATELIER_DEPLOY_HOST (e.g. root@my-vps)"
SRC="${ATELIER_DEPLOY_SRC:-.cache/atelier-src}"
SERVICE="${ATELIER_DEPLOY_SERVICE:-atelierd}"
# Unquoted on purpose: it's a list of options.
# shellcheck disable=SC2086
SSH_OPTS="${ATELIER_DEPLOY_SSH_OPTS:-}"

cd "$(git rev-parse --show-toplevel)"
command -v rsync >/dev/null || die "rsync is required"

# shellcheck disable=SC2086
remote() { ssh -o BatchMode=yes $SSH_OPTS "$HOST" "$@"; }

say "syncing source to $HOST:~/$SRC"
remote "mkdir -p '$SRC'"
# Tracked plus untracked-but-not-ignored files, so uncommitted work ships and target/ and node_modules/ don't.
# A file you delete locally lingers remotely until the next full wipe (rm -rf the SRC dir); harmless for builds.
git ls-files -z --cached --others --exclude-standard --deduplicate |
  # shellcheck disable=SC2086
  rsync -az --from0 --files-from=- -e "ssh -o BatchMode=yes $SSH_OPTS" ./ "$HOST:$SRC/"

say "building atelierd on the VPS"
# A non-interactive ssh shell doesn't load rustup's profile.
remote "set -e; [ -f \"\$HOME/.cargo/env\" ] && . \"\$HOME/.cargo/env\"; command -v cargo >/dev/null || { echo 'no Rust toolchain on the VPS: curl https://sh.rustup.rs | sh' >&2; exit 1; }; cd '$SRC' && cargo build --release -p atelier-server" ||
  die "the build failed (log above)"

say "installing and restarting $SERVICE"
remote "set -e; S=; [ \"\$(id -u)\" = 0 ] || S=sudo; \$S install -m 0755 '$SRC/target/release/atelierd' /usr/local/bin/atelierd.new && \$S mv /usr/local/bin/atelierd.new /usr/local/bin/atelierd && \$S systemctl restart '$SERVICE'"

say "waiting for the server"
up=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if health="$(remote 'curl -fsS http://127.0.0.1:7747/health' 2>/dev/null)"; then up=1; break; fi
  sleep 1
done
if [ -z "$up" ]; then
  remote "journalctl -u '$SERVICE' -n 30 --no-pager 2>/dev/null || sudo journalctl -u '$SERVICE' -n 30 --no-pager" >&2 || true
  die "the server didn't come back up (log above)"
fi
say "up: $health"

if [ -n "$LOGS" ]; then
  # shellcheck disable=SC2086
  exec ssh $SSH_OPTS "$HOST" "journalctl -fu '$SERVICE' -n 20"
fi
