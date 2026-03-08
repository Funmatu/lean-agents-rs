#!/bin/bash
PORT=8081 cargo run --release &
SERVER_PID=$!
sleep 5 # Wait for server to start

# Run curl in background
echo "Starting curl task..."
curl -N -d '{"task": "Build a simple to-do list API in Rust"}' -H 'Content-Type: application/json' http://localhost:8081/v1/agent/stream &
CURL_PID=$!

sleep 3 # Wait 3 seconds, let the engine start
echo "Simulating client disconnect (Ctrl+C)..."
kill -INT $CURL_PID
wait $CURL_PID 2>/dev/null

sleep 2 # Allow server to process cancellation and print logs
echo "Shutting down server..."
kill -TERM $SERVER_PID
wait $SERVER_PID 2>/dev/null
echo "Test finished."
