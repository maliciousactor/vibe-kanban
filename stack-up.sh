#!/bin/bash
set -e

echo "=== Stopping any existing processes on ports 3000, 3001, 3003, 3008 ==="
# Kill processes on all known ports
lsof -ti:3000 | xargs kill -9 2>/dev/null || true
lsof -ti:3001 | xargs kill -9 2>/dev/null || true
lsof -ti:3003 | xargs kill -9 2>/dev/null || true
lsof -ti:3008 | xargs kill -9 2>/dev/null || true

# Kill any lingering vite or server processes
pkill -f "vite" 2>/dev/null || true
pkill -f "target/debug/server" 2>/dev/null || true

sleep 2

# Verify ports are free before proceeding
echo "Verifying ports are free..."
for port in 3000 3001 3003 3008; do
    if lsof -ti:$port &>/dev/null; then
        echo "❌ Port $port still in use, forcing kill..."
        lsof -ti:$port | xargs kill -9 2>/dev/null || true
        sleep 1
    fi
done

echo ""
echo "=== Installing dependencies ==="
# Install with pnpm if needed
if [ ! -d "node_modules" ] || [ "pnpm-lock.yaml" -nt "node_modules" ]; then
    pnpm install
else
    echo "Dependencies already up to date"
fi

echo ""
echo "=== Checking for sqlx-cli ==="
if ! command -v sqlx &> /dev/null; then
    echo "Installing sqlx-cli..."
    cargo install sqlx-cli --features sqlite
else
    echo "sqlx-cli is already installed"
fi

echo ""
echo "=== Preparing database ==="
# Prepare SQLx database
pnpm run prepare-db

echo ""
echo "=== Starting backend on port 3001 ==="
# Start backend with proper env vars in background
export VK_ALLOWED_ORIGINS="http://localhost:3000"
export BACKEND_PORT=3001
export DISABLE_WORKTREE_ORPHAN_CLEANUP=1
export RUST_LOG=debug

# Kill any existing server first
pkill -f "target/debug/server" 2>/dev/null || true
sleep 1

cargo run --bin server &
BACKEND_PID=$!
echo "Backend started with PID: $BACKEND_PID"

# Wait for backend to be ready
echo "Waiting for backend to start..."
for i in {1..30}; do
    if curl -s http://localhost:3001/health &>/dev/null; then
        echo "✅ Backend is ready on port 3001"
        break
    fi
    if ! kill -0 $BACKEND_PID 2>/dev/null; then
        echo "❌ Backend process died unexpectedly"
        exit 1
    fi
    sleep 1
done

echo ""
echo "=== Starting frontend on port 3000 ==="
# Start frontend
export VITE_VK_SHARED_API_BASE="http://localhost:3001"
pnpm run frontend:dev &
FRONTEND_PID=$!
echo "Frontend started with PID: $FRONTEND_PID"

# Wait for frontend to be ready
echo "Waiting for frontend to start..."
for i in {1..30}; do
    if curl -s http://localhost:3000 &>/dev/null; then
        echo "✅ Frontend is ready on port 3000"
        break
    fi
    if ! kill -0 $FRONTEND_PID 2>/dev/null; then
        echo "❌ Frontend process died unexpectedly"
        exit 1
    fi
    sleep 1
done

echo ""
echo "=========================================="
echo "=== Stack is running ==="
echo "=========================================="
echo ""
echo "Frontend: http://localhost:3000"
echo "Backend:  http://localhost:3001"
echo ""
echo "Backend PID:  $BACKEND_PID"
echo "Frontend PID: $FRONTEND_PID"
echo ""
echo "To stop: ./stack-down.sh"
echo "=========================================="
