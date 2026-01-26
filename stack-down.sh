#!/bin/bash
set -e

echo "=== Stopping vibe-kanban stack ==="

# Function to kill process on a port
kill_port() {
    local port=$1
    if lsof -ti:$port &>/dev/null; then
        echo "Stopping processes on port $port..."
        lsof -ti:$port | xargs kill -9 2>/dev/null || true
        sleep 0.5
        # Force kill if still running
        if lsof -ti:$port &>/dev/null; then
            lsof -ti:$port | xargs kill -9 2>/dev/null || true
            sleep 0.5
        fi
    fi
}

# Kill processes on all known ports
kill_port 3000  # frontend
kill_port 3001  # backend
kill_port 3003  # alternate backend
kill_port 3008  # alternate backend (from pnpm run dev)

echo "Checking for lingering vite processes in vibe-kanban..."
# More aggressive vite kill
pkill -f "vite.*vibe-kanban" 2>/dev/null || true
pkill -f "vite --port" 2>/dev/null || true

echo "Stopping cargo-watch processes..."
pkill -f "cargo watch" 2>/dev/null || true

# Additional cleanup - kill any remaining server processes
pkill -f "target/debug/server" 2>/dev/null || true

sleep 1

echo "=== Verification ==="
# Verify ports are free
for port in 3000 3001 3003 3008; do
    if lsof -ti:$port &>/dev/null; then
        echo "❌ Port $port is still in use"
        lsof -ti:$port | xargs kill -9 2>/dev/null || true
        sleep 0.5
        if lsof -ti:$port &>/dev/null; then
            echo "❌ Failed to free port $port"
            exit 1
        else
            echo "✅ Port $port freed"
        fi
    else
        echo "✅ Port $port is free"
    fi
done

echo ""
echo "=== Stack stopped ==="
echo "All vibe-kanban processes have been terminated."
