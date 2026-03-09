# 🤖 AIコーダーへの実装指示書：ステートマシンの仕様補完とベンチマーク基盤の堅牢化

## 1. プロジェクト背景とデータ駆動（DDD）による課題分析

現在、`lean-agents-rs` プロジェクトにおける RTX 3090 でのマルチモデル・ベンチマーク（`scripts/benchmark_rtx3090.sh`）実行中に、以下の2つの独立したクリティカルなバグが観測されている。

**【課題A：Rustドメイン層における不正な状態遷移例外】**

* **観測データ (Log):** `data: {"type":"workflow_escalated","reason":"Invalid state transition: invalid state transition from Planning to ToolCalling { return_to: Planning }","task_id":null}`
* **Root Cause:** Orchestratorエージェントが、タスク分解を行う前に前提知識を調べるため `Planning` フェーズで Tool Call（検索）をリクエストした。しかし `src/domain/state.rs` の `WorkflowState::can_transition_to` メソッドにおいて、`Planning` -> `ToolCalling` への遷移ルールが明示的に定義されておらず、ステートマシンがクラッシュした。

**【課題B：シェルスクリプトのパイプライン崩壊とレポート初期化問題】**

* **観測データ:** エスカレーション発生時、スクリプトが異常終了し、次回の再実行時に前回までのベンチマーク結果（Markdown）が消滅している。
* **Root Cause:** 1. スクリプト冒頭で `set -euo pipefail` を宣言しているため、エージェントが一度も発言（`agent_spoke`）せずにエラー終了した場合、`grep '"type":"agent_spoke"'` が `Exit Code 1` を返し、スクリプト全体が即死する。
2. スクリプト冒頭で `> "$REPORT_FILE"`（上書き）を使用しているため、再実行時に過去の結果がすべて初期化されてしまう。

---

## 2. 仕様駆動開発（SDD）に基づく要件定義

本システムの堅牢性を担保するため、以下の仕様を満たすこと。

**仕様 1: ステートマシンの拡張 (Rust)**

* `Planning` フェーズは、外部情報の調査を伴う可能性があるため、`ToolCalling` 状態への一時的な遷移をドメインとして正式に許可する。
* 変更対象: `src/domain/state.rs` の `can_transition_to` メソッド。

**仕様 2: レポートファイルの永続化 (Bash)**

* ベンチマークレポート（`benchmark_report.md`）は、ファイルが**存在しない場合のみ**ヘッダーを初期化し、存在する場合は追記（Append）を継続する仕様とする。

**仕様 3: パイプライン・セーフな解析ロジック (Bash)**

* ログの解析処理（`TOTAL_CHARS` の計算）において、検索対象の文字列が存在しなくてもスクリプトがクラッシュしない「パイプライン・セーフ」な実装を導入する。

---

## 3. テスト駆動開発（TDD）に基づく実装手順

AIコーダーは、以下のStep 1〜4の順序で厳格に実装とテストを行うこと。

### Step 1: Rustドメイン層のテスト追加と実装

1. **テストの記述 (Red):**
`src/domain/state.rs` 内の `mod tests` にある `valid_transitions` テスト、または新規テスト `planning_to_toolcalling_transition` を作成し、以下をアサートせよ。
```rust
let tool_state = WorkflowState::ToolCalling { return_to: Box::new(WorkflowState::Planning) };
assert!(WorkflowState::Planning.can_transition_to(&tool_state));

```


2. **実装 (Green):**
同ファイル内の `can_transition_to` メソッドにおける `Planning` の遷移ルールに以下を追加せよ。
```rust
(WorkflowState::Planning, WorkflowState::ToolCalling { .. }) => true,

```


3. **検証 (Refactor):**
`cargo test domain::state::tests` を実行し、テストがPASSすることを証明せよ。

### Step 2: ベンチマークスクリプトの堅牢化 (1) - レポート初期化防止

`scripts/benchmark_rtx3090.sh` の `Markdown Report Initialization` ブロック（41行目付近）を以下のように書き換えよ。

```bash
# --- Markdown Report Initialization ---
if [[ ! -f "$REPORT_FILE" ]]; then
    echo "# RTX 3090 Benchmark Results" > "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo "| Model | Engine | Peak VRAM (MB) | Time (sec) | Approx System T/s | Status | Output Log |" >> "$REPORT_FILE"
    echo "|---|---|---|---|---|---|---|" >> "$REPORT_FILE"
else
    echo "[INFO] Existing report file found. Appending results..."
fi

```

### Step 3: ベンチマークスクリプトの堅牢化 (2) - パイプライン・セーフティ

同スクリプト内の `TOTAL_CHARS` および `APPROX_TOKENS` の計算ブロック（135行目付近）を、`set -e` の監視下でも絶対に落ちないロジック（`if` 文による `grep -q` の事前チェック）に書き換えよ。

```bash
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

```

### Step 4: キャッシュの強制破棄と再ビルド

コードの修正が完了したら、前回のように古いDockerイメージのキャッシュが効いてしまうのを防ぐため、必ず以下のコマンドを実行して最新のRustバイナリをコンテナにデプロイせよ。

```bash
docker compose -f docker-compose.bench.yml down -v || true
docker rmi lean-agents-rs-lean-agents:latest -f || true
docker compose -f docker-compose.bench.yml build --no-cache lean-agents

```

## 4. 完了条件

* `cargo test` の全件PASS。
* Orchestratorエージェントが `Planning` フェーズで正常に検索ツールを呼び出せること。
* エージェントが発言せずにエスカレーションしてもスクリプトが異常終了せず、次のモデルのテストに正常に移行すること。
* ベンチマーク結果がMarkdownファイルに追記され続けること。

作業を開始してください。
