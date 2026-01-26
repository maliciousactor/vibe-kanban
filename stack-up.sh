#!/bin/bash
set -e

echo "=== Stopping any existing processes on ports 3000 and 3001 ==="
# Kill processes on ports 3000 and 3001
lsof -ti:3000 | xargs kill -9 2>/dev/null || true
lsof -ti:3001 | xargs kill -9 2>/dev/null || true
sleep 1

echo "=== Installing dependencies ==="
# Install with pnpm if needed
if [ ! -d "node_modules" ] || [ "pnpm-lock.yaml" -nt "node_modules" ]; then
    pnpm install
else
    echo "Dependencies already up to date"
fi

echo "=== Checking for sqlx-cli ==="
if ! command -v sqlx &> /dev/null; then
    echo "Installing sqlx-cli..."
    cargo install sqlx-cli --features sqlite
else
    echo "sqlx-cli is already installed"
fi

echo "=== Preparing database ==="
# Prepare SQLx database
pnpm run prepare-db

echo "=== Starting backend on port 3001 ==="
# Start backend with proper env vars in background
export VK_ALLOWED_ORIGINS="http://localhost:3000"
export BACKEND_PORT=3001
export DISABLE_WORKTREE_ORPHAN_CLEANUP=1
export RUST_LOG=debug

cargo run --bin server &
BACKEND_PID=$!
echo "Backend started with PID: $BACKEND_PID"
sleep 3

echo "=== Starting frontend on port 3000 ==="
# Start frontend
export VITE_VK_SHARED_API_BASE="http://localhost:3001"
pnpm run frontend:dev &
FRONTEND_PID=$!
echo "Frontend started with PID: $FRONTEND_PID"

echo ""
echo "=== Stack is running ==="
echo "Frontend: http://localhost:3000"
echo "Backend: http://localhost:3001"
echo ""
echo "Backend PID: $BACKEND_PID"
echo "Frontend PID: $FRONTEND_PID"
echo ""
echo "To stop: kill $BACKEND_PID $FRONTEND_PID"
