#!/bin/bash
# debug.sh

echo "Starting debug session..."
echo "Testing MCP server and client communication"
echo "----------------------------------------"

export RUST_LOG=debug

# Create logs directory
mkdir -p logs

# Run the server with output capture
cargo run --bin server > logs/server_stdout.log 2> logs/server_stderr.log &
SERVER_PID=$!

# Wait a moment for server to start
sleep 1

# Run the client with output capture
cargo run --bin client -- list-resources > logs/client_stdout.log 2> logs/client_stderr.log

# Kill the server
kill $SERVER_PID

# Show the logs
echo "=== Server STDOUT ==="
cat logs/server_stdout.log
echo -e "\n=== Server STDERR ==="
cat logs/server_stderr.log
echo -e "\n=== Client STDOUT ==="
cat logs/client_stdout.log
echo -e "\n=== Client STDERR ==="
cat logs/client_stderr.log