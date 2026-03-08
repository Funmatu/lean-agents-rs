#!/usr/bin/env bash
# profile_uma.sh — UMA bandwidth profiling for ASUS GX10
#
# Measures GPU memory/PCIe bandwidth usage during concurrent task execution
# to determine optimal MAX_CONCURRENT_TASKS for the SGLang + lean-agents-rs stack.
#
# Usage:
#   ./scripts/profile_uma.sh [--log-dir DIR]

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LOG_DIR="${PROJECT_ROOT}"
LOG_FILE=""
DMON_PID=""

# Parse optional --log-dir
while [[ $# -gt 0 ]]; do
    case "$1" in
        --log-dir)
            LOG_DIR="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

LOG_FILE="${LOG_DIR}/uma_bandwidth.log"

# ---------------------------------------------------------------------------
# Prerequisite checks
# ---------------------------------------------------------------------------
for cmd in nvidia-smi uv awk; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "ERROR: Required command '$cmd' not found. Please install it first." >&2
        exit 1
    fi
done

# ---------------------------------------------------------------------------
# Cleanup handler — ensures nvidia-smi dmon is stopped on exit
# ---------------------------------------------------------------------------
cleanup() {
    if [[ -n "$DMON_PID" ]] && kill -0 "$DMON_PID" 2>/dev/null; then
        echo ""
        echo "Stopping nvidia-smi dmon (PID: $DMON_PID)..."
        kill "$DMON_PID" 2>/dev/null || true
        wait "$DMON_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 1. Start background GPU profiling
# ---------------------------------------------------------------------------
echo "========================================"
echo " UMA Bandwidth Profiler"
echo " Target: ASUS GX10 (273 GB/s UMA)"
echo "========================================"
echo ""
echo "Starting nvidia-smi dmon (memory mode)..."
echo "Log file: ${LOG_FILE}"
echo ""

nvidia-smi dmon -s m -d 1 > "$LOG_FILE" 2>&1 &
DMON_PID=$!
echo "nvidia-smi dmon started (PID: ${DMON_PID})"

# Give dmon a moment to initialize
sleep 1

# ---------------------------------------------------------------------------
# 2. Run concurrent load test
# ---------------------------------------------------------------------------
echo ""
echo "Running concurrent load test (5 tasks)..."
echo "----------------------------------------"

cd "$PROJECT_ROOT"
uv run tests/test_runner.py --run-concurrent

echo ""
echo "Load test completed."

# ---------------------------------------------------------------------------
# 3. Stop profiling and analyze
# ---------------------------------------------------------------------------
# Stop dmon (cleanup trap will also fire, but explicit stop gives cleaner output)
if kill -0 "$DMON_PID" 2>/dev/null; then
    kill "$DMON_PID" 2>/dev/null || true
    wait "$DMON_PID" 2>/dev/null || true
    echo "nvidia-smi dmon stopped."
fi
DMON_PID=""  # Prevent double-kill in trap

echo ""
echo "========================================"
echo " Bandwidth Analysis"
echo "========================================"
echo ""

# Parse the log: dynamically find column indices for rxpci and txpci
# nvidia-smi dmon -s m outputs a header line starting with "# gpu" followed by column names.
# We find the positions of rxpci and txpci from that header.
awk '
BEGIN {
    rxpci_col = 0
    txpci_col = 0
    rx_sum = 0; rx_max = 0; rx_count = 0
    tx_sum = 0; tx_max = 0; tx_count = 0
}

# Header line: detect column positions
/^# gpu/ {
    for (i = 1; i <= NF; i++) {
        gsub(/^#\s*/, "", $i)
        if ($i == "rxpci") rxpci_col = i - 1  # offset for leading "# "
        if ($i == "txpci") txpci_col = i - 1
    }
    next
}

# Skip comment/header lines
/^#/ { next }

# Skip empty lines
NF == 0 { next }

# Data lines
{
    if (rxpci_col > 0 && rxpci_col <= NF) {
        val = $rxpci_col + 0
        if (val >= 0) {
            rx_sum += val
            rx_count++
            if (val > rx_max) rx_max = val
        }
    }
    if (txpci_col > 0 && txpci_col <= NF) {
        val = $txpci_col + 0
        if (val >= 0) {
            tx_sum += val
            tx_count++
            if (val > tx_max) tx_max = val
        }
    }
}

END {
    if (rx_count == 0 && tx_count == 0) {
        print "No data collected. Is the server running and GPU accessible?"
        exit 1
    }

    printf "%-12s %10s %10s %10s\n", "Metric", "Max (MB/s)", "Avg (MB/s)", "Samples"
    printf "%-12s %10s %10s %10s\n", "--------", "----------", "----------", "-------"

    if (rx_count > 0) {
        printf "%-12s %10.1f %10.1f %10d\n", "rxpci", rx_max, rx_sum / rx_count, rx_count
    } else {
        printf "%-12s %10s %10s %10s\n", "rxpci", "N/A", "N/A", "0"
    }

    if (tx_count > 0) {
        printf "%-12s %10.1f %10.1f %10d\n", "txpci", tx_max, tx_sum / tx_count, tx_count
    } else {
        printf "%-12s %10s %10s %10s\n", "txpci", "N/A", "N/A", "0"
    }

    print ""

    if (rx_count > 0 || tx_count > 0) {
        total_max = rx_max + tx_max
        total_avg = 0
        if (rx_count > 0) total_avg += rx_sum / rx_count
        if (tx_count > 0) total_avg += tx_sum / tx_count
        printf "%-12s %10.1f %10.1f\n", "Total", total_max, total_avg
        print ""
        printf "UMA theoretical bandwidth: 273,000 MB/s (273 GB/s)\n"
        printf "Peak utilization:          %.2f%%\n", (total_max / 273000.0) * 100
        printf "Average utilization:       %.2f%%\n", (total_avg / 273000.0) * 100
    }
}
' "$LOG_FILE"

echo ""
echo "Raw log saved to: ${LOG_FILE}"
echo "Done."
