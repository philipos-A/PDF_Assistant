#!/usr/bin/env bash
# Start the Rust backend (port 8000) and the Vite frontend (port 5173).
# Ctrl-C stops both.
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null; then
  echo "Rust is not installed. See https://rustup.rs" >&2
  exit 1
fi
if [ ! -d frontend/node_modules ]; then
  echo "Frontend not set up. Run: make setup" >&2
  exit 1
fi
if [ ! -f backend/.env ]; then
  echo "warning: backend/.env not found; copy backend/.env.example and add ANTHROPIC_API_KEY" >&2
fi

trap 'kill 0' EXIT INT TERM

(cd backend && cargo run --release) &
(cd frontend && npm run dev -- --port 5173 --strictPort) &

echo
echo "  Backend:  http://localhost:8000/api/health"
echo "  Frontend: http://localhost:5173"
echo
wait
