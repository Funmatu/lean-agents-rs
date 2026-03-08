# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "httpx",
#     "httpx-sse",
#     "rich",
# ]
# ///
"""
lean-agents-rs CLI Client with Human-in-the-Loop (HITL) intervention support.

Usage:
    uv run tools/cli_client/client.py [--host HOST] [--port PORT]

This tool connects to the lean-agents-rs API over SSE, renders agent
conversations with role-based coloring, and supports interactive HITL
intervention when the workflow escalates.
"""
import asyncio
import json
import sys
from typing import Optional

import httpx
from httpx_sse import aconnect_sse
from rich.console import Console
from rich.panel import Panel
from rich.prompt import Prompt
from rich.table import Table
from rich.text import Text

console = Console()

# ----- Configuration -----

DEFAULT_HOST = "localhost"
DEFAULT_PORT = 8080

# Role-based color palette
ROLE_COLORS = {
    "Orchestrator": "cyan",
    "Architect": "magenta",
    "Programmer": "green",
    "DevilsAdvocate": "red",
    "System": "yellow",
}

# Resumable phases for HITL intervention
RESUME_PHASES = ["Planning", "Designing", "Implementing", "Reviewing"]


def base_url(host: str, port: int) -> str:
    return f"http://{host}:{port}"


def render_event(data: dict) -> Optional[str]:
    """Render an SSE event dict to the console. Returns task_id if escalated."""
    event_type = data.get("type")

    if event_type == "workflow_started":
        console.print("\n[bold green]▶ Workflow Started![/bold green]")

    elif event_type == "state_changed":
        to_state = data["to"]
        # Detect CompressingContext transitions (serialized as object with key)
        if isinstance(to_state, dict) and "CompressingContext" in str(to_state):
            console.print(
                "[bold bright_yellow]"
                "[🧹 Context Compression Triggered - Summarizing History...]"
                "[/bold bright_yellow]"
            )
        elif isinstance(data["from"], dict) and "CompressingContext" in str(data["from"]):
            console.print(
                f"[bold bright_yellow]"
                f"[🧹 Context Compression Complete - Resuming {to_state}]"
                f"[/bold bright_yellow]"
            )
        else:
            console.print(
                f"[bold blue]🔄 State:[/bold blue] {data['from']} ➔ {to_state}"
            )

    elif event_type == "agent_thinking":
        role = data["role"]
        color = ROLE_COLORS.get(role, "white")
        console.print(f"[{color}]🤔 {role} is thinking...[/{color}]")

    elif event_type == "agent_spoke":
        role = data["role"]
        content = data["content"]
        color = ROLE_COLORS.get(role, "white")
        display = content if len(content) < 800 else content[:800] + "\n… (truncated)"
        console.print(Panel(display, title=role, border_style=color))

    elif event_type == "tool_call_executed":
        role = data["role"]
        query = data["query"]
        console.print(
            f"[bold yellow]🛠️  Tool Call by {role}:[/bold yellow] '{query}'"
        )

    elif event_type == "workflow_completed":
        console.print(
            "[bold green]✅ Workflow Completed Successfully![/bold green]\n"
        )

    elif event_type == "workflow_escalated":
        reason = data.get("reason", "Unknown")
        task_id = data.get("task_id")
        console.print()
        console.print(
            Panel(
                f"[bold]Reason:[/bold] {reason}",
                title="🚨 ESCALATED — Human Intervention Required",
                border_style="bold red",
            )
        )
        return task_id  # may be None if no task_id

    elif event_type == "ping":
        pass  # heartbeat, silent

    else:
        console.print(f"[dim]Unknown event: {event_type}[/dim]")

    return None


async def prompt_intervention() -> tuple[str, str]:
    """Prompt the user for HITL intervention message and resume phase."""
    console.print(
        "[bold yellow]⏸  Entering HITL Intervention Mode[/bold yellow]"
    )
    console.print(
        "[dim]The workflow has paused and is waiting for your guidance.[/dim]\n"
    )

    # Show available phases
    table = Table(title="Available Resume Phases", show_lines=False)
    table.add_column("#", style="bold", width=4)
    table.add_column("Phase", style="cyan")
    for i, phase in enumerate(RESUME_PHASES, 1):
        table.add_row(str(i), phase)
    console.print(table)
    console.print()

    # Get message (run blocking input on a thread to keep asyncio happy)
    message = await asyncio.to_thread(
        Prompt.ask, "[bold cyan]Your intervention message[/bold cyan]"
    )

    # Get phase selection
    while True:
        choice = await asyncio.to_thread(
            Prompt.ask,
            f"[bold cyan]Resume phase (1-{len(RESUME_PHASES)})[/bold cyan]",
        )
        try:
            idx = int(choice) - 1
            if 0 <= idx < len(RESUME_PHASES):
                return message, RESUME_PHASES[idx]
        except ValueError:
            pass
        console.print("[red]Invalid selection. Please try again.[/red]")


async def send_intervention(
    host: str, port: int, task_id: str, message: str, resume_state: str
):
    """POST the intervention payload to the /v1/agent/intervene endpoint."""
    url = f"{base_url(host, port)}/v1/agent/intervene"
    payload = {
        "task_id": task_id,
        "message": message,
        "resume_state": resume_state,
    }
    console.print(
        f"[dim]Sending intervention → {resume_state}...[/dim]"
    )
    async with httpx.AsyncClient(timeout=30.0) as client:
        resp = await client.post(url, json=payload)
        if resp.status_code < 300:
            console.print(
                f"[bold green]✅ Intervention accepted — resuming from {resume_state}[/bold green]\n"
            )
        else:
            console.print(
                f"[bold red]❌ Intervention failed ({resp.status_code}): {resp.text}[/bold red]"
            )


async def stream_task(host: str, port: int, task_prompt: str):
    """
    Send a task to the stream endpoint and render SSE events.
    If the workflow escalates with a task_id, enter HITL intervention mode
    and continue listening after the intervention is sent.
    """
    url = f"{base_url(host, port)}/v1/agent/stream"
    timeout = httpx.Timeout(600.0)  # Long timeout for LLM inference

    async with httpx.AsyncClient(timeout=timeout) as client:
        async with aconnect_sse(
            client, "POST", url, json={"task": task_prompt}
        ) as event_source:
            async for sse in event_source.aiter_sse():
                if not sse.data:
                    continue

                try:
                    data = json.loads(sse.data)
                except json.JSONDecodeError:
                    console.print(f"[red]Failed to parse: {sse.data}[/red]")
                    continue

                escalated_task_id = render_event(data)

                if escalated_task_id is not None:
                    # Enter HITL mode
                    message, resume_state = await prompt_intervention()
                    await send_intervention(
                        host, port, escalated_task_id, message, resume_state
                    )
                    # Continue SSE loop — the backend will resume and
                    # keep sending events on this same stream.


async def interactive_loop(host: str, port: int):
    """Main interactive REPL loop."""
    console.print(
        Panel(
            "[bold]lean-agents-rs CLI Client[/bold]\n"
            f"Connected to {base_url(host, port)}\n"
            "Type a task to send it to the agent pipeline.\n"
            "Type [bold cyan]quit[/bold cyan] or press [bold cyan]Ctrl+C[/bold cyan] to exit.",
            border_style="bright_blue",
        )
    )

    while True:
        console.print()
        task = await asyncio.to_thread(
            Prompt.ask, "[bold bright_blue]📝 Task[/bold bright_blue]"
        )

        if task.strip().lower() in ("quit", "exit", "q"):
            console.print("[dim]Goodbye![/dim]")
            break

        if not task.strip():
            console.print("[yellow]Empty task, skipped.[/yellow]")
            continue

        try:
            await stream_task(host, port, task.strip())
        except httpx.ConnectError:
            console.print(
                f"[bold red]Connection refused.[/bold red] "
                f"Is the server running at {base_url(host, port)}?"
            )
        except httpx.ReadTimeout:
            console.print("[bold red]Read timeout — server took too long.[/bold red]")


def parse_args() -> tuple[str, int]:
    """Minimal argv parser for --host and --port."""
    host = DEFAULT_HOST
    port = DEFAULT_PORT
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--host" and i + 1 < len(args):
            host = args[i + 1]
            i += 2
        elif args[i] == "--port" and i + 1 < len(args):
            port = int(args[i + 1])
            i += 2
        else:
            i += 1
    return host, port


def main():
    host, port = parse_args()
    try:
        asyncio.run(interactive_loop(host, port))
    except KeyboardInterrupt:
        console.print("\n[dim]Interrupted — shutting down cleanly.[/dim]")
        sys.exit(0)


if __name__ == "__main__":
    main()
