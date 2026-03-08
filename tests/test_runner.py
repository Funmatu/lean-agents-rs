# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "httpx",
#     "httpx-sse",
#     "rich",
# ]
# ///
import asyncio
import json
import sys
import httpx
from httpx_sse import aconnect_sse
from rich.console import Console
from rich.panel import Panel

console = Console()

API_URL = "http://localhost:8080/v1/agent/stream"

# 役割ごとの色設定
ROLE_COLORS = {
    "Orchestrator": "cyan",
    "Architect": "magenta",
    "Programmer": "green",
    "DevilsAdvocate": "red",
    "System": "yellow",
}


async def run_task(task_name: str, task_prompt: str, task_id: int = 1):
    """1つのタスクをAPIに投げ、SSEストリームを受信して描画する"""
    console.print(f"\n[bold yellow]--- Test {task_id}: {task_name} ---[/bold yellow]")
    console.print(f"Task: {task_prompt}")

    timeout = httpx.Timeout(600.0)  # LLMの推論待ちで切断されないよう長めに設定

    async with httpx.AsyncClient(timeout=timeout) as client:
        try:
            async with aconnect_sse(
                client, "POST", API_URL, json={"task": task_prompt}
            ) as event_source:
                async for sse in event_source.aiter_sse():
                    if not sse.data:
                        continue

                    try:
                        data = json.loads(sse.data)
                        event_type = data.get("type")

                        if event_type == "workflow_started":
                            console.print(
                                f"[bold green]▶ Workflow Started![/bold green]"
                            )

                        elif event_type == "state_changed":
                            console.print(
                                f"[bold blue]🔄 State Changed:[/bold blue] {data['from']} ➔ {data['to']}"
                            )

                        elif event_type == "agent_thinking":
                            role = data["role"]
                            color = ROLE_COLORS.get(role, "white")
                            console.print(
                                f"[{color}]🤔 {role} is thinking...[/{color}]"
                            )

                        elif event_type == "agent_spoke":
                            role = data["role"]
                            content = data["content"]
                            color = ROLE_COLORS.get(role, "white")
                            # 長すぎる出力は少し省略して表示
                            display_content = (
                                content
                                if len(content) < 500
                                else content[:500] + "...\n(truncated)"
                            )
                            console.print(
                                Panel(
                                    display_content, title=f"{role}", border_style=color
                                )
                            )

                        elif event_type == "tool_call_executed":
                            role = data["role"]
                            query = data["query"]
                            console.print(
                                f"[bold yellow]🛠️  Tool Call by {role}:[/bold yellow] Searching for '{query}'"
                            )

                        elif event_type == "workflow_completed":
                            console.print(
                                f"[bold green]✅ Workflow Completed Successfully![/bold green]"
                            )

                        elif event_type == "workflow_escalated":
                            reason = data.get("reason", "Unknown")
                            console.print(
                                f"[bold red]🚨 Workflow Escalated (Failed/Stopped):[/bold red] {reason}"
                            )

                    except json.JSONDecodeError:
                        console.print(f"[red]Failed to parse JSON: {sse.data}[/red]")

        except Exception as e:
            console.print(f"[bold red]Connection Error:[/bold red] {e}")


async def test_concurrent_execution():
    """セマフォ（並行制御）のテスト：5つのタスクを同時に投げる"""
    console.print("\n[bold yellow]--- Running Concurrent Tasks Test ---[/bold yellow]")
    tasks = [
        run_task("Concurrent 1", "Rustのメモリ管理について100文字で教えて", 1),
        run_task("Concurrent 2", "Axumの特徴を100文字で教えて", 2),
        run_task("Concurrent 3", "Tokioとは何か100文字で教えて", 3),
        run_task("Concurrent 4", "SGLangのメリットを100文字で教えて", 4),
        run_task("Concurrent 5", "Dockerの利点を100文字で教えて", 5),
    ]
    await asyncio.gather(*tasks)


async def main():
    while True:
        console.print("\n[bold cyan]Select a test to run:[/bold cyan]")
        console.print("1. [Basic] 簡単な挨拶テスト (SSE通信確認)")
        console.print("2. [Tool Call] 未知の情報を含むテスト (Tavily検索の発動確認)")
        console.print(
            "3. [Escalation] 不可能/複雑すぎるタスク (エスカレーション/自己修正ループの確認)"
        )
        console.print("4. [Concurrency] 5タスク同時実行 (セマフォの動作確認)")
        console.print("q. Quit")

        choice = input("Enter choice: ")

        if choice == "1":
            await run_task(
                "Basic Flow", "「こんにちは」とだけ言って、すぐに完了させてください。"
            )
        elif choice == "2":
            await run_task(
                "Tool Call Test",
                "Rustの最新の安定版バージョン番号を検索して、教えてください。",
            )
        elif choice == "3":
            # 2Bモデルなら少し複雑なタスクを投げると高確率でJSONパースエラーやフォーマット違反を起こしてエスカレーションします
            await run_task(
                "Escalation Test",
                "Linuxカーネルのスケジューラのソースコードを完全にRustで書き直して出力してください。",
            )
        elif choice == "4":
            await test_concurrent_execution()
        elif choice.lower() == "q":
            break
        else:
            print("Invalid choice.")


if __name__ == "__main__":
    if "--run-concurrent" in sys.argv:
        asyncio.run(test_concurrent_execution())
    else:
        asyncio.run(main())
