#!/bin/bash
# Test script for distributed mode (local, without Docker)
# Usage: ./scripts/test-distributed.sh

set -e

echo "=== Fusillade Distributed Mode Test ==="
echo ""

# Build first
echo "Building fusillade..."
cargo build --release 2>/dev/null || cargo build

FUSILLADE="./target/release/fusillade"
if [ ! -f "$FUSILLADE" ]; then
    FUSILLADE="./target/debug/fusillade"
fi

echo "Using binary: $FUSILLADE"
echo ""

# Start controller in background
echo "Starting controller on :9000..."
$FUSILLADE controller --listen 0.0.0.0:9000 &
CONTROLLER_PID=$!
sleep 2

# Check if controller is running
if ! kill -0 $CONTROLLER_PID 2>/dev/null; then
    echo "ERROR: Controller failed to start"
    exit 1
fi
echo "Controller running (PID: $CONTROLLER_PID)"

# Start 2 workers in background
echo "Starting workers..."
$FUSILLADE worker --connect 127.0.0.1:9001 &
WORKER1_PID=$!
$FUSILLADE worker --connect 127.0.0.1:9001 &
WORKER2_PID=$!
sleep 2

echo "Workers running (PIDs: $WORKER1_PID, $WORKER2_PID)"
echo ""

# Check controller status
echo "Checking controller API..."
STATUS=$(curl -s http://localhost:9000/api/stats 2>/dev/null || echo "FAILED")
if [ "$STATUS" = "FAILED" ]; then
    echo "ERROR: Controller API not responding"
    kill $CONTROLLER_PID $WORKER1_PID $WORKER2_PID 2>/dev/null
    exit 1
fi
echo "Controller API OK: $STATUS"
echo ""

# Cleanup
cleanup() {
    echo ""
    echo "Cleaning up..."
    kill $CONTROLLER_PID $WORKER1_PID $WORKER2_PID 2>/dev/null || true
    echo "Done!"
}
trap cleanup EXIT

echo "=== All components running ==="
echo ""
echo "Dashboard: http://localhost:9000/dashboard"
echo "API:       http://localhost:9000/api/stats"
echo ""
echo "Press Ctrl+C to stop..."
wait
