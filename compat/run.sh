#!/usr/bin/env bash
# Runs the Bolt compatibility suites (Python and JavaScript) against a fresh
# emulator on a random port with a throwaway database.
#
#   compat/run.sh            # both suites
#   compat/run.sh python     # just one
#
# EMU_COMPAT_DATABASE_URL runs the emulator on another database (e.g. an empty
# Postgres database) instead of a temporary SQLite file.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
suites=("${@:-python js}")
work="$(mktemp -d)"
trap 'kill "${pid:-}" 2>/dev/null || true; rm -rf "$work"' EXIT

port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
export EMU_URL="http://127.0.0.1:$port"
export EMU_ADMIN_TOKEN="compat-$RANDOM$RANDOM"

cargo build --quiet --manifest-path "$root/Cargo.toml" --bin slack-emu
DATABASE_URL="${EMU_COMPAT_DATABASE_URL:-sqlite://$work/emu.db}" EMU_ADMIN_TOKEN="$EMU_ADMIN_TOKEN" BIND_ADDR="127.0.0.1:$port" PUBLIC_URL="$EMU_URL" \
  "$root/target/debug/slack-emu" >"$work/server.log" 2>&1 &
pid=$!
for _ in $(seq 50); do curl -sf "$EMU_URL/health" >/dev/null && break; sleep 0.2; done

status=0
for suite in ${suites[@]}; do
  case "$suite" in
    python)
      echo "== Bolt for Python"
      if [ ! -d "$work/venv" ]; then
        python3 -m venv "$work/venv"
        "$work/venv/bin/pip" install --quiet -r "$root/compat/python/requirements.txt"
      fi
      (cd "$root/compat/python" && "$work/venv/bin/python" -m pytest -q -p no:cacheprovider) || status=1
      ;;
    js)
      echo "== Bolt for JavaScript"
      (cd "$root/compat/js" && npm ci --silent --no-audit --no-fund && npm test --silent) || status=1
      ;;
    *) echo "unknown suite: $suite" >&2; exit 2 ;;
  esac
done

if [ "$status" -ne 0 ]; then
  echo "== emulator log (last 50 lines)"
  tail -50 "$work/server.log"
fi
exit "$status"
