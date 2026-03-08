# 🤖 AIコーダーへの実装指示：【Phase 3】 コンテキストの要約圧縮・リセット機能（Context Compression）

プロジェクトの目標を「実稼働（Production）レベルでの連続稼働」に引き上げます。
現在のアーキテクチャでは、タスクが長引くと `ContextGraph` の `messages` が単調増加し続け、いずれ限界を迎えます。パフォーマンステストを行う前に、**Phase 3** として、会話履歴が一定量を超えた際に Orchestrator に要約を行わせ、コンテキストをリセットする「Context Compression」機構を実装します。

ただし、私たちが使用するLLM（Qwen3.5-27B）は最大262kトークンの巨大なコンテキストウィンドウを持っています。固定値で小さく制限するのではなく、環境変数で柔軟に制御できるアーキテクチャにします。

## 🛠️ 実装手順

### 3-1. Domain層の更新 (`src/domain/`)

1. **`state.rs`:**
* `WorkflowState` Enum に `CompressingContext { return_to: Box<WorkflowState> }` を追加せよ。
* `can_transition_to` を更新し、すべての状態から `CompressingContext` への遷移、および `CompressingContext` から任意の元の状態への遷移を許可せよ。


2. **`context.rs`:**
* `ContextGraph` に、現在の全メッセージの総文字数を計算するメソッド `total_content_length(&self) -> usize` を追加せよ。
* メッセージ履歴をリセットするメソッド `reset_with_summary(&mut self, summary: String)` を実装せよ。これは内部の `messages` を `clear` し、先頭に `Message::new(AgentRole::System, format!("[System Checkpoint Summary]\n{}", summary))` を挿入する処理とする。



### 3-2. AppState と Engine層の更新

1. **`src/server/state.rs` & `src/main.rs`:**
* 環境変数 `MAX_CONTEXT_LENGTH` を読み込む処理を追加せよ（パースできない場合や未指定時のデフォルトは `120000` 文字とする）。
* `AppState` に `max_context_length: usize` を追加し、起動時にセットせよ。


2. **`src/engine/mod.rs`:**
* `Engine::run` の引数に `max_context_length: usize` を追加（Router側から渡す）せよ。
* ステートマシンループ内において、`Planning`, `Designing`, `Implementing`, `Reviewing` などの主要な処理を行う**直前**に、`context.total_content_length() > max_context_length` をチェックするロジックを挿入せよ。


3. もし閾値を超えていた場合：
* 現在の `context.state()` を `return_to` に保持した上で、`WorkflowState::CompressingContext` へ `transition_to` し、`StateChanged` イベントを発火せよ。
* `continue;` を呼び出してループを回し、`CompressingContext` アームに入らせよ。


4. `WorkflowState::CompressingContext` のマッチアームを実装せよ：
* クライアントに `AgentThinking { role: AgentRole::Orchestrator }` を発火。
* Orchestrator を用いて、LLMに特別タスクを投げる。この際、通常の `execute_with_tool_support` ではなく、一時的なプロンプトとして「*You are the Orchestrator. The context is getting too long. Summarize the entire discussion history above. Include the original goal, finalized architectural decisions, current implementation status, and remaining issues. Do not use tools. Output only the summary.*」を末尾に付与して `llm.chat_completion` を直接叩くこと（コンテキストは現在の `build_messages` を活用）。
* 結果が得られたら、`context.reset_with_summary(summary)` を呼び出す。
* `return_to` に保持していた元の状態へ `transition_to` し、ループを継続せよ。



### 3-3. CLIクライアントの更新 (`tools/cli_client/client.py`)

* 新たな状態遷移（`CompressingContext` への出入り）がストリームで流れてきた際、コンソールに目立つ色で `[🧹 Context Compression Triggered - Summarizing History...]` のように表示し、ユーザーに何が起きているか視覚的に伝わるようにせよ。

### ✅ Phase 3 完了条件

* `cargo check` と `cargo test` がすべて通ること。
* アプリケーションの起動時に `MAX_CONTEXT_LENGTH=2000` のように極端に小さい値を設定してCLIから少し長めのタスクを投げ、意図的に文字数を溢れさせて、自動的に要約が走り、履歴が圧縮されて元のフェーズに復帰する様子（ログ表示）が確認できること。

実装とテストが完了しましたら、修正されたコード（diff）と、テストで圧縮がトリガーされた際のCLIの動作ログを報告してください。