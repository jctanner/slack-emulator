#!/usr/bin/env bash
# The full test matrix: the Rust tests and the Bolt compatibility suites, on
# SQLite and on Postgres. Postgres runs in a throwaway container (podman or
# docker) unless EMU_TEST_POSTGRES points at a server. Run it locally, or as
# a CI job's only step.
#
#   scripts/test-matrix.sh             # everything
#   scripts/test-matrix.sh sqlite      # one half
#   scripts/test-matrix.sh postgres
#   EMU_TEST_POSTGRES=postgres://user:pw@host/db scripts/test-matrix.sh postgres
#
# With your own server, the Rust tests create (and leave) `emu_test_*`
# databases, and the compat suites use the database in the URL, so point it
# at a scratch server.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
which="${1:-all}"

# The server embeds the web UI, so it needs a build of it.
if [ ! -f web/dist/index.html ]; then
  echo "== Building the web UI"
  (cd web && npm ci --silent --no-audit --no-fund && npm run build)
fi

sqlite() {
  echo "== Rust tests (SQLite)"
  cargo test --quiet
  echo "== Bolt compatibility (SQLite)"
  compat/run.sh
}

postgres() {
  local compat_url
  if [ -z "${EMU_TEST_POSTGRES:-}" ]; then
    local engine name port
    engine="$(command -v podman || command -v docker)" || {
      echo "Postgres tests need podman or docker, or EMU_TEST_POSTGRES" >&2
      exit 2
    }
    name="slack-emu-test-pg-$$"
    "$engine" run -d --rm --name "$name" -e POSTGRES_PASSWORD=emu -p 127.0.0.1::5432 \
      "${EMU_POSTGRES_IMAGE:-docker.io/library/postgres:16}" >/dev/null
    # Expanded now: the function's locals are gone when the trap runs.
    trap "'$engine' rm -f '$name' >/dev/null" EXIT
    port="$("$engine" port "$name" 5432 | head -1 | sed 's/.*://')"
    # Ready over TCP means past the image's first-start initialisation.
    for _ in $(seq 120); do
      "$engine" exec "$name" pg_isready -q -h 127.0.0.1 -U postgres && break
      sleep 0.5
    done
    export EMU_TEST_POSTGRES="postgres://postgres:emu@127.0.0.1:$port/postgres"
    "$engine" exec "$name" createdb -U postgres emu_compat
    compat_url="postgres://postgres:emu@127.0.0.1:$port/emu_compat"
  else
    compat_url="$EMU_TEST_POSTGRES"
  fi
  echo "== Rust tests (Postgres)"
  cargo test --quiet
  echo "== Bolt compatibility (Postgres)"
  EMU_COMPAT_DATABASE_URL="$compat_url" compat/run.sh
}

case "$which" in
  all) sqlite; postgres ;;
  sqlite) sqlite ;;
  postgres) postgres ;;
  *) echo "usage: $0 [all|sqlite|postgres]" >&2; exit 2 ;;
esac
echo "== All passed"
