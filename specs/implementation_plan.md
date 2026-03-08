# 🤖 AIコーダーへの実装指示書：LLM推論エンジン間のAPI互換性レイヤー実装

## 1. プロジェクト背景と現在の課題

本プロジェクト `lean-agents-rs` は、Rustで構築された自律型マルチエージェントシステムです。SGLang をプライマリ推論エンジンとして開発されてきましたが、現在、RTX 3090 環境でのベンチマーク検証のために `llama.cpp` (`llama-server`) との統合テストを行っています。

しかし、`llama.cpp` バックエンドを使用した場合、初期タスク投入後の Orchestrator エージェントの初回推論時に以下のエラーが発生してワークフローがクラッシュします。

**エラーログ:**

```json
data: {"type":"workflow_escalated","reason":"LLM client error: HTTP 400 Bad Request: {\"error\":{\"code\":400,\"message\":\"Assistant response prefill is incompatible with enable_thinking.\",\"type\":\"invalid_request_error\"}}","task_id":null}

```

## 2. データ駆動の分析 (Root Cause Analysis)

1. **ドメインモデルの現状:**
システムはユーザーからの初回タスクを `AgentRole::Orchestrator` の発言として `ContextGraph` に登録します。これはマルチエージェントシステムの内部ステートとして「Orchestratorの初期目標」を表現する正しいドメインモデリングです。
2. **SGLang と llama.cpp の仕様差異:**
* **SGLang:** チャット履歴の末尾が `assistant` ロールであっても、それを Assistant Prefill（回答の書き出し）として認識し、シームレスに推論を継続します。
* **llama.cpp:** 最新版では `enable_thinking` 機能との競合により、OpenAI互換APIエンドポイントにおいて「末尾が `assistant` ロールであること（Assistant Prefill）」を厳格に拒否し、HTTP 400 エラーを返却します。



## 3. 仕様駆動開発 (SDD) に基づく要件定義

**【絶対的な制約】**
ドメイン層 (`src/domain/` 内の `ContextGraph`, `Message`, `AgentRole`) およびステートマシン (`src/engine/mod.rs`) のコアロジックは**一切変更してはならない**。ドメインの純粋性を保つこと。

**【実装要件】**
API通信を構築するアダプター層（`src/agents/mod.rs` の `build_messages` メソッド、または `src/client/llm.rs`）において、推論エンジンに依存しない堅牢な互換性レイヤーを実装せよ。

1. `ContextGraph` から LLM API 用の `Vec<ChatMessage>` を構築する際、メッセージの連続性や末尾のロールを検査・変換するロジックを導入すること。
2. LLMAPI に送信される `messages` 配列の最後は、必ず `user` または `system` で終わるように保証すること。
3. もし `AgentRole::Orchestrator` 等のエージェント自身の発言（`assistant` ロールにマッピングされる）が末尾にある場合は、推論エンジンが拒否しない形（例：ダミーの `user` メッセージの挿入、または初回タスクのみ `user` ロールとしてAPIに送信する等）に適切にマッピングすること。

## 4. テスト駆動開発 (TDD) に基づく実装手順

以下のステップに沿って実装および検証を行ってください。

### Step 1: 既存テストの実行と状態確認

* `cargo test` を実行し、既存のドメインロジックおよび `src/agents/mod.rs` のテスト（`build_messages_without_volatile_context` 等）がパスすることを確認せよ。

### Step 2: 失敗する単体テストの追加

* `src/agents/mod.rs` のテストモジュール内に、`llama.cpp` の制約をシミュレートするテストを追加せよ。
* **テスト名:** `test_build_messages_prevents_assistant_prefill`
* **条件:** `ContextGraph` に `AgentRole::Orchestrator` のメッセージのみが存在する場合。
* **アサーション:** 構築された `Vec<ChatMessage>` の最後の要素の `role` が `"assistant"` ではないこと（`"user"` であること）を検証する。



### Step 3: 互換性レイヤーの実装

* `src/agents/mod.rs` の `Agent::build_messages` トレイトメソッドを改修し、追加したテストがパスするようにせよ。
* 修正方針の例：
履歴をループして `ChatMessage` を生成した後、`messages.last().map(|m| m.role.as_str()) == Some("assistant")` の条件に合致する場合、`role: "user", content: "Please proceed with the task."` のようなトリガーメッセージを Append する。
※前回の簡易な修正では失敗したため、llama.cppのパーサーが確実に認識できるフォーマットを検討すること。あるいは、初回のタスク投入時（メッセージ配列の長さが2の場合など）に限り、送信元のロールマッピングを動的に `user` に変更するアプローチも検討せよ。

### Step 4: ビルドと単体テストの通過

* `cargo build` および `cargo test` が完全にパスすることを保証せよ。

### Step 5: 統合テスト（ベンチマークスクリプトの実行）

* 提供されている `scripts/benchmark_rtx3090.sh` を使用して、実際に `llama.cpp` バックエンドを立ち上げて結合テストを行え。
* **重要:** スクリプト内の `docker compose` コマンドには必ず `--build` フラグを付与し、修正したRustバイナリがコンテナにデプロイされるようにすること。

## 5. 期待される成果物

1. `src/agents/mod.rs` の改修コード（互換性レイヤーの完全な実装）。
2. 同ファイル内に追加された単体テストコード。
3. 実行結果のログ（`HTTP 400 Bad Request` が解消され、Orchestrator が `Planning` フェーズの推論を正常に完了し、エージェントループが回ることの証明）。

さあ、システムの堅牢性をさらに一段階引き上げるための実装を開始してください。
