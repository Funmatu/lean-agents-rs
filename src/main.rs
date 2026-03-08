use std::env;
use std::process;

use lean_agents_rs::client::llm::SgLangClient;
use lean_agents_rs::client::search::TavilyClient;
use lean_agents_rs::domain::context::ContextGraph;
use lean_agents_rs::domain::state::WorkflowState;
use lean_agents_rs::engine::Engine;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let sglang_url = env::var("SGLANG_URL").unwrap_or_else(|_| {
        eprintln!("Warning: SGLANG_URL not set, using default http://localhost:30000");
        "http://localhost:30000".to_string()
    });

    let model = env::var("SGLANG_MODEL").unwrap_or_else(|_| "Qwen3.5-27B".to_string());

    let tavily_key = env::var("TAVILY_API_KEY").unwrap_or_else(|_| {
        eprintln!("Warning: TAVILY_API_KEY not set, search functionality will fail");
        String::new()
    });

    let task = env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .join(" ");

    if task.is_empty() {
        eprintln!("Usage: lean-agents-rs <task description>");
        eprintln!();
        eprintln!("Environment variables:");
        eprintln!("  SGLANG_URL       SGLang API endpoint (default: http://localhost:30000)");
        eprintln!("  SGLANG_MODEL     Model name (default: Qwen3.5-27B)");
        eprintln!("  TAVILY_API_KEY   Tavily search API key");
        eprintln!("  RUST_LOG         Log level (default: info)");
        process::exit(1);
    }

    let llm = SgLangClient::new(&sglang_url, &model);
    let search = TavilyClient::new(&tavily_key);
    let engine = Engine::new();
    let mut context = ContextGraph::new();

    tracing::info!("Starting lean-agents-rs with task: {}", task);
    tracing::info!("SGLang endpoint: {}", sglang_url);
    tracing::info!("Model: {}", model);

    match engine.run(&mut context, &llm, &search, &task).await {
        Ok(()) => match context.state() {
            WorkflowState::Completed => {
                tracing::info!("Workflow completed successfully");
                println!("\n=== Workflow Complete ===");
                for msg in context.messages() {
                    println!("[{}] {}", msg.sender, msg.content);
                }
            }
            WorkflowState::Escalated => {
                tracing::warn!("Workflow escalated (deadlock prevention triggered)");
                println!("\n=== Workflow Escalated ===");
                println!("The agents could not reach consensus within the iteration limit.");
                for msg in context.messages() {
                    println!("[{}] {}", msg.sender, msg.content);
                }
                process::exit(2);
            }
            state => {
                tracing::error!("Unexpected final state: {:?}", state);
                process::exit(3);
            }
        },
        Err(e) => {
            tracing::error!("Engine error: {}", e);
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    }
}
