#!/usr/bin/env bash
# scripts/benchmark_rtx3090.sh — Multi-model benchmark for lean-agents-rs on RTX 3090

set -euo pipefail

# --- Configuration ---
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_DIR="${PROJECT_ROOT}/bench_logs"
mkdir -p "$LOG_DIR"
REPORT_FILE="${LOG_DIR}/benchmark_report.md"

TASK_PROMPT="RustでシンプルなLRU Cache（Least Recently Used）を実装し、テストコードも書いてください。"

# モデル定義配列: "表示名 | エンジン | DL_URL_OR_MODEL_ID | FILE_NAME_OR_MEMFRAC | CtxSize"
MODELS=(
    "Unsloth_9B_Q4|llama|https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf|Qwen3.5-9B-Q4_K_M.gguf|16384"
    "Unsloth_27B_Q4|llama|https://huggingface.co/unsloth/Qwen3.5-27B-GGUF/resolve/main/Qwen3.5-27B-Q4_K_M.gguf|Qwen3.5-27B-Q4_K_M.gguf|8192"
    "SGLang_9B|sglang|Qwen/Qwen3.5-9B|0.85|-"
)

# --- Cleanup Trap ---
cleanup() {
    echo -e "\n[INFO] Cleaning up containers..."
    docker compose -f docker-compose.bench.yml down >/dev/null 2>&1 || true
    rm -f docker-compose.bench.yml
    if [[ -n "${VRAM_PID:-}" ]] && kill -0 "$VRAM_PID" 2>/dev/null; then
        kill "$VRAM_PID"
    fi
}
trap cleanup EXIT

# --- Markdown Report Initialization ---
if [[ ! -f "$REPORT_FILE" ]]; then
    echo "# RTX 3090 Benchmark Results" > "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo "| Model | Engine | Peak VRAM (MB) | Time (sec) | Approx System T/s | Status | Output Log |" >> "$REPORT_FILE"
    echo "|---|---|---|---|---|---|---|" >> "$REPORT_FILE"
else
    echo "[INFO] Existing report file found. Appending results..."
fi

# --- Main Benchmark Loop ---
for MODEL_ENTRY in "${MODELS[@]}"; do
    IFS='|' read -r DISPLAY_NAME ENGINE URL_OR_ID ARG1 ARG2 <<< "$MODEL_ENTRY"
    DISPLAY_NAME=$(echo "$DISPLAY_NAME" | xargs)
    ENGINE=$(echo "$ENGINE" | xargs)
    URL_OR_ID=$(echo "$URL_OR_ID" | xargs)
    ARG1=$(echo "$ARG1" | xargs)
    ARG2=$(echo "$ARG2" | xargs)

    echo "=================================================="
    echo " Starting Benchmark: $DISPLAY_NAME"
    echo "=================================================="

    if [[ "$ENGINE" == "llama" ]]; then
            IMAGE="ghcr.io/ggml-org/llama.cpp:server-cuda"
            DL_URL="$URL_OR_ID"
            FILE_NAME="$ARG1"
            CTX_SIZE="$ARG2"

            ENTRYPOINT_BLOCK="    entrypoint:
          - /bin/sh
          - -c
          - |
            echo 'Initializing container...'
            apt-get update >/dev/null 2>&1 || true
            apt-get install -y curl ca-certificates >/dev/null 2>&1 || true
            mkdir -p /root/.cache/huggingface
            echo 'Downloading ${FILE_NAME} (this may take a while)...'
            curl -L -C - -o /root/.cache/huggingface/${FILE_NAME} \"${DL_URL}\"
            echo 'Starting llama-server...'
            /app/llama-server -m /root/.cache/huggingface/${FILE_NAME} --port 30000 --host 0.0.0.0 -c ${CTX_SIZE} -np 2 -ngl 99"
            
            # コンテナに渡す SGLANG_MODEL 名はダミー（llamaでは使わないため）
            ENV_MODEL_ID="dummy_model_for_llama"
        else
            IMAGE="lmsysorg/sglang:latest"
            MODEL_ID="$URL_OR_ID"
            MEM_FRAC="$ARG1"
            ENTRYPOINT_BLOCK="    command: >
          python3 -m sglang.launch_server --model-path ${MODEL_ID} --port 30000 --host 0.0.0.0 --trust-remote-code --mem-fraction-static ${MEM_FRAC}"
          
            ENV_MODEL_ID="${MODEL_ID}"
        fi

    cat <<EOF > docker-compose.bench.yml
services:
  sglang:
    image: ${IMAGE}
${ENTRYPOINT_BLOCK}
    ports:
      - "30000:30000"
    volumes:
      - model-cache:/root/.cache/huggingface
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: all
              capabilities: [ gpu ]
    healthcheck:
      test: [ "CMD", "curl", "-f", "http://localhost:30000/health" ]
      interval: 10s
      timeout: 5s
      retries: 180
      start_period: 300s

  lean-agents:
    build: .
    ports:
      - "8080:8080"
    environment:
      SGLANG_URL: http://sglang:30000
      SGLANG_MODEL: ${ENV_MODEL_ID}
      TAVILY_API_KEY: \${TAVILY_API_KEY:-""}
      MAX_CONCURRENT_TASKS: 2
      PORT: 8080
      RUST_LOG: info
      MAX_CONTEXT_LENGTH: 40000
    depends_on:
      sglang:
        condition: service_healthy
volumes:
  model-cache:
EOF

    echo "[INFO] Starting containers for $DISPLAY_NAME..."
    
    if ! docker compose -f docker-compose.bench.yml up --build -d; then
        echo "[ERROR] Failed to start containers for $DISPLAY_NAME."
        echo -e "\n--- CONTAINER LOGS (Why it crashed) ---"
        docker compose -f docker-compose.bench.yml logs sglang
        echo "---------------------------------------"
        exit 1
    fi

    echo "[INFO] Waiting for API to be ready (First run will download the model, which takes time)..."
    until curl -s http://localhost:8080/ >/dev/null 2>&1 || [[ $? == 52 ]]; do
        sleep 5
    done
    sleep 5 

    VRAM_LOG="${LOG_DIR}/${DISPLAY_NAME}_vram.log"
    rm -f "$VRAM_LOG"
    nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits -l 1 > "$VRAM_LOG" &
    VRAM_PID=$!

    echo "[INFO] Executing task..."
    START_TIME=$(date +%s)
    
    RAW_LOG="${LOG_DIR}/${DISPLAY_NAME}_raw.jsonl"
    > "$RAW_LOG" # ログファイルを初期化

    # curlをバックグラウンドで実行
    curl -s -N -X POST http://localhost:8080/v1/agent/stream \
         -H "Content-Type: application/json" \
         -d "{\"task\": \"${TASK_PROMPT}\"}" > "$RAW_LOG" &
    CURL_PID=$!

    # ログを2秒間隔で監視し、終了イベントを検知したらストリームを強制切断
    IS_SUCCESS="Unknown"
    while kill -0 $CURL_PID 2>/dev/null; do
        if grep -q '"type":"workflow_completed"' "$RAW_LOG" 2>/dev/null; then
            echo "[INFO] ✅ Workflow Completed successfully!"
            IS_SUCCESS="✅ Success"
            kill -9 $CURL_PID 2>/dev/null || true
            break
        elif grep -q '"type":"workflow_escalated"' "$RAW_LOG" 2>/dev/null; then
            echo "[INFO] 🚨 Workflow Escalated (Failed). Moving to next model..."
            IS_SUCCESS="❌ Failed"
            kill -9 $CURL_PID 2>/dev/null || true
            break
        fi
        sleep 2
    done
    wait $CURL_PID 2>/dev/null || true
         
    END_TIME=$(date +%s)
    TOTAL_TIME=$((END_TIME - START_TIME))

    kill "$VRAM_PID" 2>/dev/null || true
    PEAK_VRAM=$(sort -nr "$VRAM_LOG" 2>/dev/null | head -n 1 || echo "N/A")
    if [[ -z "$PEAK_VRAM" ]]; then PEAK_VRAM="N/A"; fi

    # 安全な文字数カウント（パイプエラー回避）
    TOTAL_CHARS=0
    if grep -q '"type":"agent_spoke"' "$RAW_LOG" 2>/dev/null; then
        TOTAL_CHARS=$(grep '"type":"agent_spoke"' "$RAW_LOG" | sed 's/data: //' | jq -r '.content' | wc -m)
    fi

    # 欠損値対策
    if [[ -z "$TOTAL_CHARS" || ! "$TOTAL_CHARS" =~ ^[0-9]+$ ]]; then
        TOTAL_CHARS=0
    fi
    APPROX_TOKENS=$((TOTAL_CHARS / 4))
    
    if [[ $TOTAL_TIME -gt 0 ]]; then
        TOKEN_PER_SEC=$(awk "BEGIN {printf \"%.1f\", $APPROX_TOKENS / $TOTAL_TIME}")
    else
        TOKEN_PER_SEC="0.0"
    fi

    echo "[RESULT] Status: ${IS_SUCCESS}, Time: ${TOTAL_TIME}s, Peak VRAM: ${PEAK_VRAM}MB, Approx T/s: ${TOKEN_PER_SEC}"

    echo "| **$DISPLAY_NAME** | $ENGINE | $PEAK_VRAM | $TOTAL_TIME | $TOKEN_PER_SEC | $IS_SUCCESS | [Log](./${DISPLAY_NAME}_raw.jsonl) |" >> "$REPORT_FILE"

    docker compose -f docker-compose.bench.yml down >/dev/null 2>&1
    sleep 3
done

echo ""
echo "=================================================="
echo " All benchmarks completed!"
echo " Results saved to: $REPORT_FILE"
cat "$REPORT_FILE"
