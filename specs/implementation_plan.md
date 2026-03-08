# 🤖 AIコーダーへの実装指示：超詳細な `README.md` の作成

プロジェクトのPhase 1およびPhase 2（ステートマシン化、HITL、コンテキスト圧縮、フォールバック、UMAプロファイリング）の実装がすべて完了しました。
これまでの集大成として、現在のリポジトリのソースコードを徹底的に精査・分析し、本システム（`lean-agents-rs`）の全貌を完璧に記述した超詳細な `README.md` を作成してください。

## 🎯 ドキュメント作成の目的と前提

* **ターゲット読者:** シニアなソフトウェアエンジニア、AIアーキテクト、および本システムを本番環境（ASUS GX10 などの UMA アーキテクチャ）で運用するインフラエンジニア。
* **トーン＆マナー:** プロフェッショナル、体系的、網羅的。
* **技術の強調:** RustのRAIIや `tokio` の非同期制御による安全性、SGLangのRadixAttention（Prefix Caching）を活かす設計、そして今回構築した「決してクラッシュしないステートマシンと自己修復機構」を強くアピールすること。

## 🔍 分析すべきソースコードの範囲

READMEを作成する前に、必ず以下のファイルを読み込み、仕様とデータの流れを完全に理解してください。

* `src/domain/`: 状態（`WorkflowState`）、エージェントの役割（`AgentRole`）、コンテキスト管理と文字数計算（`ContextGraph`）、イベント定義。
* `src/engine/`: ステートマシンのコア・ループ（`Engine::run`）、キャンセレーション、コンテキスト圧縮（`just_compressed` ガード）、HITLエスカレーション処理。
* `src/server/`: APIルーター（`/v1/agent/stream`, `/v1/agent/intervene`）、セマフォによる並行制御、`DashMap` を用いた状態保持。
* `src/client/`: SGLangとの通信クライアント、および `FallbackSearchClient`。
* `tools/cli_client/client.py`: HITL介入モードを備えたリッチなCLIツール。
* `scripts/profile_uma.sh`: UMA帯域のプロファイリングスクリプト。
* `docker-compose.yml` & `Dockerfile`: 起動構成と環境変数。

## 📝 README.md の必須構成（章立て）

以下の構成に従って、Markdown形式で出力してください。

1. **Project Overview (プロジェクト概要)**
* システムが解決する課題と、設計思想（Lean, Fast, Fault-tolerant）。


2. **Architecture & Design Philosophy (アーキテクチャと設計思想)**
* ASUS GX10 (128GB LPDDR5x UMA) に最適化された設計（並行数のセマフォ制御）。
* SGLangのRadixAttention（キャッシュ）を最大化するAppend-onlyなコンテキスト管理。
* RAIIと `CancellationToken` を用いた確実なGPUリソース解放。


3. **Key Features (主要機能)**
* **State Machine Engine:** 状態遷移ベースの推論ループ。
* **Human-in-the-Loop (HITL):** エラーや無限ループ時の安全なエスカレーションと、外部APIからの介入・復帰機構。
* **Context Compression:** `MAX_CONTEXT_LENGTH` 超過時のOrchestratorによる自動要約・コンテキスト浄化（ハルシネーションの自己修复）。
* **Fallback Search Mechanism:** Tavily枯渇時の代替手段と、品質劣化時のSystem Warning動的注入。


4. **Agents & Roles (エージェントと役割)**
* Orchestrator, Architect, Programmer, DevilsAdvocate, System, Human の各役割の解説。


5. **State Transition Flow (状態遷移フロー) 📊**
* **[必須]** Mermaid.js の `stateDiagram-v2` を用いて、`Init` -> `Planning` -> `Designing` <-> `Implementing` <-> `Reviewing` -> `Completed`、および各フェーズから `Escalated` -> `AwaitingHumanInput` へのエスカレーションと復帰、`CompressingContext` への遷移を描画すること。


6. **API Reference (API リファレンス)**
* `POST /v1/agent/stream` (SSEの仕様、ペイロード、返却イベント種類)。
* `POST /v1/agent/intervene` (ペイロード仕様と動作)。


7. **Setup & Deployment (環境構築と起動)**
* Docker Compose を用いた起動手順。
* 主要な環境変数の解説（`SGLANG_URL`, `TAVILY_API_KEY`, `MAX_CONCURRENT_TASKS`, `MAX_CONTEXT_LENGTH` 等）。


8. **Included Tools (付属ツール)**
* **CLI Client (`tools/cli_client/client.py`):** 使い方と介入モードのデモ。
* **UMA Profiler (`scripts/profile_uma.sh`):** ハードウェア限界テストの実行方法。



## ✅ 完了条件

* 指定されたすべてのコンポーネントとロジックが、ソースコードの実際の挙動と一字一句矛盾なく正確に記述されていること。
* Mermaidによる状態遷移図が正しくレンダリング可能なフォーマットで記述されていること。
* 出力はリポジトリのルートに `README.md` として保存すること。

準備ができたら、コードを精査し、最高のドキュメントを作成してください。