#!/bin/bash
set -e

echo "=== Stopping vibe-kanban stack ==="

# Kill processes by port (most reliable method for this project)
echo "Stopping processes on port 3000 (frontend)..."
lsof -ti:3000 2>/dev/null | xargs kill -9 2>/dev/null || echo "No process found on port 3000"

echo "Stopping processes on port 3001 (backend)..."
lsof -ti:3001 2>/dev/null | xargs kill -9 2>/dev/null || echo "No process found on port 3001"

# Also check for any lingering processes from previous runs
echo "Checking for lingering vite processes in vibe-kanban..."
lsof -ti:3003 2>/dev/null | xargs kill -9 2>/dev/null || true
lsof -ti:3008 2>/dev/null | xargs kill -9 2>/dev/null || true

# Kill any cargo-watch processes watching this project
echo "Stopping cargo-watch processes..."
pkill -f "cargo-watch.*vibe-kanban" 2>/dev/null || echo "No cargo-watch found"

# Kill any vite processes running from the frontend directory
echo "Stopping vite processes..."
pkill -f "vite.*frontend" 2>/dev/null || echo "No vite processes found"

# Give processes time to terminate
sleep 1

# Verify ports are free
echo ""
echo "=== Verification ==="
if lsof -ti:3000 > /dev/null 2>&1; then
    echo "WARNING: Port 3000 still in use"
else
    echo "✅ Port 3000 is free"
fi

if lsof -ti:3001 > /dev/null 2>&1; then
    echo "WARNING: Port 3001 still in use"
else
    echo "✅ Port 3001 is free"
fi

echo ""
echo "=== Stack stopped ==="
echo "All vibe-kanban processes have been terminated."
