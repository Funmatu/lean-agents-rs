# 【厳格指令】lean-agents-rs Phase 2 実装指示書

## 📌 ミッション概要

あなたは世界最高峰のRustアーキテクトです。ASUS GX10 (UMAアーキテクチャ) 環境向けに最適化された `lean-agents-rs` プロジェクトのPhase 2（堅牢化・対話・フォールバック機構）を実装します。
現在のアーキテクチャの最大の強みである「不要なメモリ確保の排除」「RadixAttentionキャッシュの保護」「RAIIによる確実なリソース解放」を絶対原則とし、1バイトたりとも無駄なアロケーションを増やさないこと。

## ⚠️ 実装の基本ルール (絶対遵守)

1. **SDD & TDDの徹底:** 必ず「ドメイン（型）の定義」→「テストコードの作成」→「実装」の順で進めること。
2. **ステップバイステップ:** 以下の `Step 1` から順番に着手すること。**1つのStepが完了し、テストがパスし、ユーザー(私)が「次へ」と指示するまで、絶対に次のStepのコードを出力しないこと。**
3. **エラーハンドリング:** `unwrap()` はテストコード以外で絶対に使用しない。必ず `AppError` にマッピングすること。

---

## 🚀 実装ステップ

### 【Step 1】 ネットワーク堅牢化（キャンセレーションとタイムアウト防止）

**課題:** クライアントがブラウザを閉じてSSEが切断されても、裏で推論が完遂するまでGPUとセマフォ枠を占有してしまう。また、Nginx環境下で長時間無音だとタイムアウトする。

#### 1-1. Domain層の更新 (Ping & Cancel Event)

* `src/domain/event.rs` の `EngineEvent` に `Ping` を追加せよ。
* `src/error.rs` (または `lib.rs` 内) の `AppError` に `Cancelled` バリアントを追加せよ。

#### 1-2. CancellationTokenの導入

* `Cargo.toml` に `tokio-util = "0.7"` を追加せよ。
* `src/engine/mod.rs` の `Engine::run` および `execute_with_tool_support` メソッドの引数に `cancel_token: tokio_util::sync::CancellationToken` を追加せよ。
* `execute_with_tool_support` 内の `agent.execute(context, llm).await` を以下のように `tokio::select!` でラップし、キャンセル検知時は即座に `AppError::Cancelled` を返すようにリファクタリングせよ。
```rust
tokio::select! {
    res = agent.execute(context, llm) => res?,
    _ = cancel_token.cancelled() => return Err(AppError::Cancelled),
}

```



#### 1-3. Router層での切断検知とPing送信

* `src/server/router.rs` の `stream_handler` にて以下を実装せよ。
1. `CancellationToken::new()` を生成し、`Engine::run` に渡す。
2. クライアント切断検知タスクをスポーンする: `tokio::spawn(async move { tx.closed().await; token.cancel(); });` (これによりSSE切断時に即座にトークンが発火する)。
3. `ReceiverStream` 変換部分で、`tokio::time::interval` を用いて、15秒間イベントがない場合に `EngineEvent::Ping` をSSEに流す非同期ストリーム拡張（`StreamExt::timeout` など）を実装せよ。



**■ Step 1 の完了条件:** `cargo test` が通過すること。モックを用いたキャンセレーションの単体テストを `engine::tests` に追加すること。

---

### 【Step 2】 Human-in-the-Loop (HITL) 機構の実装

**課題:** エスカレーション時にタスクが破棄されるのを防ぎ、人間が介入してリカバリできるようにする。

#### 2-1. Domain層の更新 (状態とロール)

* `src/domain/agent.rs` の `AgentRole` に `Human` を追加せよ。
* `src/domain/state.rs` の `WorkflowState` に `AwaitingHumanInput` を追加せよ。
* 遷移ルール: `Escalated -> AwaitingHumanInput`, `AwaitingHumanInput -> Planning`, `AwaitingHumanInput -> Designing` などを許可するように `can_transition_to` を更新し、テストせよ。

#### 2-2. 状態保持と介入エンドポイント (Router & State)

* `src/server/state.rs` の `AppState` に、中断されたタスクを保持する `active_interventions: Arc<DashMap<String, mpsc::Sender<String>>>` を追加せよ（`dashmap` クレートを使用）。
* `src/engine/mod.rs` において、`Escalated` に到達した際、即座に終了するのではなく、ユニークな `task_id` を発行し、受信用のチャネル (`mpsc::channel`) を `active_interventions` に登録せよ。その後 `AwaitingHumanInput` に遷移してチャネルの受信を待機 (`recv().await`) せよ。
* 受信した人間のメッセージを `ContextGraph` に追加し、指定されたフェーズ（例：`Designing`）へ復帰させるロジックを実装せよ。
* `src/server/router.rs` に `POST /v1/agent/intervene` (ペイロード: `{ task_id, message, resume_state }`) エンドポイントを追加し、該当タスクのチャネルにメッセージを送信する処理を実装せよ。

**■ Step 2 の完了条件:** `dashmap` を用いた状態保持と、`Escalated` から復帰する流れの単体テストが通ること。

---

### 【Step 3】 検索APIフォールバック機構と情報劣化警告

**課題:** Tavily APIの無料枠が枯渇した際、自動的に代替APIに切り替え、かつLLMが情報劣化（短いスニペット）で混乱するのを防ぐ。

#### 3-1. Domain / Client 層の拡張

* `src/client/search.rs` の `SearchResult` に `is_fallback: bool` (または `quality` enum) を追加せよ。
* `FallbackSearchClient` 構造体を実装せよ。これは `clients: Vec<Box<dyn SearchClient>>` を保持し、`search` メソッド内で優先順位順にループを回し、エラー（レートリミットやHTTP 429）が出たら次のクライアントへフォールバックする機能を持つ。

#### 3-2. 動的プロンプト（警告）の注入

* `src/engine/mod.rs` において、検索結果を `volatile_context` にセットする際、結果の `is_fallback` が `true` であった場合、結果文字列の先頭に以下のシステム警告を動的に結合せよ。
*"[System Warning] This search result is a fallback short snippet. If details are insufficient, refine your search query or proceed with current knowledge."*

**■ Step 3 の完了条件:** `MockSearchClient` を用いて、1つ目が失敗し2つ目が成功するフォールバックのテストを作成し、パスさせること。

---

### 【Step 4】 実機パフォーマンステスト・スクリプトの作成

**課題:** UMA帯域の限界を実機 (ASUS GX10) でプロファイリングするためのツール整備。

#### 4-1. Bashプロファイリングスクリプト

* リポジトリルートに `scripts/profile_uma.sh` を作成せよ。
* 以下の処理を自動化するスクリプトを記述せよ。
1. `nvidia-smi dmon -s m -d 1 > uma_bandwidth.log &` をバックグラウンド起動。
2. Pythonスクリプト (`test_runner.py` の同時実行テスト関数など) を起動。
3. Pythonスクリプト終了後、`nvidia-smi dmon` をkill。
4. ログから `rxpci` と `txpci` (または該当するメモリスループット値) の最大値と平均値を `awk` で抽出してターミナルに表示。



**■ Step 4 の完了条件:** シェルスクリプトが正しく実行権限つきで作成され、文法エラーがないこと。

---

## 🎯 アクション指示

AIコーダーよ、指示は理解しましたか？
まずは **【Step 1】 ネットワーク堅牢化** の実装内容（特に `tx.closed().await` を用いたキャンセレーションの仕組み）について、Rustの型やライフサイクルで問題が起きないかあなたの設計見解を述べ、実装を開始するための許可を私に求めてください。一気にコードを吐き出すことは厳禁です。

---