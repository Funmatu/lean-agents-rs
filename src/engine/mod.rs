use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::agents::architect::ArchitectAgent;
use crate::agents::devils_advocate::DevilsAdvocateAgent;
use crate::agents::orchestrator::OrchestratorAgent;
use crate::agents::programmer::ProgrammerAgent;
use crate::agents::Agent;
use crate::client::llm::LlmClient;
use crate::client::search::SearchClient;
use crate::domain::agent::AgentRole;
use crate::domain::context::ContextGraph;
use crate::domain::event::EngineEvent;
use crate::domain::message::Message;
use crate::domain::state::WorkflowState;
use crate::error::AppError;
use crate::parser::{self, ParsedOutput};

const MAX_ITERATIONS: u32 = 3;
const MAX_TOOL_CALLS_PER_TURN: u32 = 3;
const MAX_PARSE_RETRIES: u32 = 3;

pub struct Engine {
    orchestrator: OrchestratorAgent,
    architect: ArchitectAgent,
    programmer: ProgrammerAgent,
    devils_advocate: DevilsAdvocateAgent,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            orchestrator: OrchestratorAgent,
            architect: ArchitectAgent,
            programmer: ProgrammerAgent,
            devils_advocate: DevilsAdvocateAgent,
        }
    }

    /// Emit an event. If the receiver is dropped (e.g. SSE client disconnected),
    /// log a warning but do NOT abort the engine — the workflow continues.
    async fn emit(tx: &mpsc::Sender<EngineEvent>, event: EngineEvent) {
        if tx.send(event).await.is_err() {
            warn!("Event receiver dropped, continuing without event delivery");
        }
    }

    /// Handle escalation: generate a single task_id, register it in the DashMap,
    /// emit events, and block until human intervention is received.
    async fn handle_escalation(
        context: &mut ContextGraph,
        tx: &mpsc::Sender<EngineEvent>,
        active_interventions: std::sync::Arc<dashmap::DashMap<String, tokio::sync::mpsc::Sender<(String, WorkflowState)>>>,
        reason: &str,
    ) -> Result<(), AppError> {
        // 1. Generate a single task_id, create the channel, and register in DashMap
        let task_id = uuid::Uuid::new_v4().to_string();
        let (intervene_tx, mut intervene_rx) = mpsc::channel(1);
        active_interventions.insert(task_id.clone(), intervene_tx);

        // 2. Transition to Escalated and emit the event (with the same task_id)
        context.transition_to(WorkflowState::Escalated)?;
        Self::emit(tx, EngineEvent::WorkflowEscalated {
            reason: reason.into(),
            task_id: Some(task_id.clone()),
        }).await;

        // 3. Transition to AwaitingHumanInput
        let from = context.state().clone();
        context.transition_to(WorkflowState::AwaitingHumanInput)?;
        Self::emit(tx, EngineEvent::StateChanged {
            from,
            to: WorkflowState::AwaitingHumanInput,
        }).await;

        tracing::info!("Workflow Escalated. Awaiting intervention on task_id: {}", task_id);

        // 4. Block until human input arrives, then resume
        if let Some((human_message, resume_state)) = intervene_rx.recv().await {
            tracing::info!("Received human intervention, resuming to {:?}", resume_state);
            context.add_message(Message::new(AgentRole::Human, &human_message));
            let from = context.state().clone();
            context.transition_to(resume_state.clone())?;
            Self::emit(tx, EngineEvent::StateChanged {
                from,
                to: resume_state,
            }).await;
        }

        Ok(())
    }

    /// Run the full workflow engine loop, emitting EngineEvents via `tx`.
    pub async fn run(
        &self,
        context: &mut ContextGraph,
        llm: &dyn LlmClient,
        search: &dyn SearchClient,
        task: &str,
        tx: &mpsc::Sender<EngineEvent>,
        cancel_token: tokio_util::sync::CancellationToken,
        active_interventions: std::sync::Arc<dashmap::DashMap<String, tokio::sync::mpsc::Sender<(String, WorkflowState)>>>,
    ) -> Result<(), AppError> {
        let mut design_iterations = 0u32;
        let mut review_iterations = 0u32;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    warn!("Workflow cancelled by cancellation token");
                    return Err(AppError::Cancelled);
                }
                res = async {
                    match context.state().clone() {
                        WorkflowState::Init => {
                            Self::emit(tx, EngineEvent::WorkflowStarted {
                                task: task.to_string(),
                            }).await;
                            let from = context.state().clone();
                            context.transition_to(WorkflowState::Planning)?;
                            Self::emit(tx, EngineEvent::StateChanged {
                                from,
                                to: WorkflowState::Planning,
                            }).await;
                            context.add_message(Message::new(AgentRole::Orchestrator, task));
                            info!("Engine started: Init -> Planning");
                            Ok(false)
                        }
                        WorkflowState::Planning => {
                            Self::emit(tx, EngineEvent::AgentThinking {
                                role: AgentRole::Orchestrator,
                            }).await;
                            let plan = self.execute_with_tool_support(
                                &self.orchestrator,
                                context,
                                llm,
                                search,
                                tx,
                                cancel_token.clone(),
                                active_interventions.clone(),
                            ).await?;
                            Self::emit(tx, EngineEvent::AgentSpoke {
                                role: AgentRole::Orchestrator,
                                content: plan.clone(),
                            }).await;
                            context.add_message(Message::new(AgentRole::Orchestrator, &plan));

                            let from = context.state().clone();
                            context.transition_to(WorkflowState::Designing)?;
                            Self::emit(tx, EngineEvent::StateChanged {
                                from,
                                to: WorkflowState::Designing,
                            }).await;
                            info!("Planning -> Designing");
                            Ok(false)
                        }
                        WorkflowState::Designing => {
                            if design_iterations >= MAX_ITERATIONS {
                                warn!("Design phase exceeded MAX_ITERATIONS, escalating");
                                Self::handle_escalation(
                                    context, tx, active_interventions.clone(),
                                    "Design phase exceeded max iterations",
                                ).await?;
                                design_iterations = 0;
                                return Ok(false);
                            }

                            Self::emit(tx, EngineEvent::AgentThinking {
                                role: AgentRole::Architect,
                            }).await;
                            let design = self.execute_with_tool_support(
                                &self.architect,
                                context,
                                llm,
                                search,
                                tx,
                                cancel_token.clone(),
                                active_interventions.clone(),
                            ).await?;
                            Self::emit(tx, EngineEvent::AgentSpoke {
                                role: AgentRole::Architect,
                                content: design.clone(),
                            }).await;
                            context.add_message(Message::new(AgentRole::Architect, &design));

                            Self::emit(tx, EngineEvent::AgentThinking {
                                role: AgentRole::DevilsAdvocate,
                            }).await;
                            let review = self.execute_with_tool_support(
                                &self.devils_advocate,
                                context,
                                llm,
                                search,
                                tx,
                                cancel_token.clone(),
                                active_interventions.clone(),
                            ).await?;
                            Self::emit(tx, EngineEvent::AgentSpoke {
                                role: AgentRole::DevilsAdvocate,
                                content: review.clone(),
                            }).await;
                            context.add_message(Message::new(AgentRole::DevilsAdvocate, &review));

                            if parser::is_approval(&review) {
                                info!("Design approved by DevilsAdvocate");
                                let from = context.state().clone();
                                context.transition_to(WorkflowState::Implementing)?;
                                Self::emit(tx, EngineEvent::StateChanged {
                                    from,
                                    to: WorkflowState::Implementing,
                                }).await;
                                info!("Designing -> Implementing");
                                Ok(false)
                            } else {
                                design_iterations += 1;
                                info!("Design iteration {}: revision requested", design_iterations);
                                Ok(false)
                            }
                        }
                        WorkflowState::Implementing => {
                            Self::emit(tx, EngineEvent::AgentThinking {
                                role: AgentRole::Programmer,
                            }).await;
                            let implementation = self.execute_with_tool_support(
                                &self.programmer,
                                context,
                                llm,
                                search,
                                tx,
                                cancel_token.clone(),
                                active_interventions.clone(),
                            ).await?;
                            Self::emit(tx, EngineEvent::AgentSpoke {
                                role: AgentRole::Programmer,
                                content: implementation.clone(),
                            }).await;
                            context.add_message(Message::new(AgentRole::Programmer, &implementation));

                            let from = context.state().clone();
                            context.transition_to(WorkflowState::Reviewing)?;
                            Self::emit(tx, EngineEvent::StateChanged {
                                from,
                                to: WorkflowState::Reviewing,
                            }).await;
                            info!("Implementing -> Reviewing");
                            Ok(false)
                        }
                        WorkflowState::Reviewing => {
                            if review_iterations >= MAX_ITERATIONS {
                                warn!("Review phase exceeded MAX_ITERATIONS, escalating");
                                Self::handle_escalation(
                                    context, tx, active_interventions.clone(),
                                    "Review phase exceeded max iterations",
                                ).await?;
                                review_iterations = 0;
                                return Ok(false);
                            }

                            Self::emit(tx, EngineEvent::AgentThinking {
                                role: AgentRole::DevilsAdvocate,
                            }).await;
                            let review = self.execute_with_tool_support(
                                &self.devils_advocate,
                                context,
                                llm,
                                search,
                                tx,
                                cancel_token.clone(),
                                active_interventions.clone(),
                            ).await?;
                            Self::emit(tx, EngineEvent::AgentSpoke {
                                role: AgentRole::DevilsAdvocate,
                                content: review.clone(),
                            }).await;
                            context.add_message(Message::new(AgentRole::DevilsAdvocate, &review));

                            if parser::is_approval(&review) {
                                info!("Implementation approved by DevilsAdvocate");
                                let from = context.state().clone();
                                context.transition_to(WorkflowState::Completed)?;
                                Self::emit(tx, EngineEvent::StateChanged {
                                    from,
                                    to: WorkflowState::Completed,
                                }).await;
                                Self::emit(tx, EngineEvent::WorkflowCompleted).await;
                                info!("Workflow completed successfully");
                                Ok(false)
                            } else {
                                let from = context.state().clone();
                                context.transition_to(WorkflowState::Implementing)?;
                                Self::emit(tx, EngineEvent::StateChanged {
                                    from,
                                    to: WorkflowState::Implementing,
                                }).await;
                                review_iterations += 1;
                                info!("Review iteration {}: rework requested", review_iterations);
                                Ok(false)
                            }
                        }
                        WorkflowState::Escalated => {
                            // Escalation is now fully handled inline by handle_escalation().
                            // This arm should not be reached in normal operation.
                            warn!("Unexpected entry into Escalated match arm");
                            Ok(true)
                        }
                        WorkflowState::AwaitingHumanInput => {
                            // AwaitingHumanInput is now handled inline by handle_escalation().
                            // This arm should not be reached in normal operation.
                            warn!("Unexpected entry into AwaitingHumanInput match arm");
                            Ok(true)
                        }
                        WorkflowState::Completed => {
                            // Exit loop successfully
                            return Ok::<bool, AppError>(true);
                        }
                        WorkflowState::ToolCalling { .. } => {
                            // This shouldn't be the top-level state evaluated here
                            Err(AppError::StateTransition(
                                "Invalid state loop hit ToolCalling directly".into(),
                            ))
                        }
                    }
                } => {
                    // Propagate errors; break if the state handler signals completion
                    if res? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Execute an agent with tool call interception and self-correction.
    async fn execute_with_tool_support(
        &self,
        agent: &dyn Agent,
        context: &mut ContextGraph,
        llm: &dyn LlmClient,
        search: &dyn SearchClient,
        tx: &mpsc::Sender<EngineEvent>,
        cancel_token: tokio_util::sync::CancellationToken,
        active_interventions: std::sync::Arc<dashmap::DashMap<String, tokio::sync::mpsc::Sender<(String, WorkflowState)>>>,
    ) -> Result<String, AppError> {
        let current_state = context.state().clone();
        let mut tool_calls = 0u32;
        let mut parse_retries = 0u32;

        loop {
            let raw = agent.execute(context, llm).await?;

            let output = match parser::parse_agent_output(&raw) {
                Ok(parsed) => {
                    parse_retries = 0;
                    parsed
                }
                Err(e) => {
                    parse_retries += 1;
                    if parse_retries > MAX_PARSE_RETRIES {
                        warn!(
                            "Agent {:?} exceeded max parse retries ({}), escalating",
                            agent.role(),
                            MAX_PARSE_RETRIES
                        );
                        context.clear_volatile_context();
                        Self::handle_escalation(
                            context,
                            tx,
                            active_interventions.clone(),
                            &format!(
                                "Agent {:?} failed to produce valid output after {} retries",
                                agent.role(),
                                MAX_PARSE_RETRIES
                            ),
                        ).await?;
                        parse_retries = 0;
                        continue;
                    }
                    warn!(
                        "Agent {:?} parse error (retry {}/{}): {}",
                        agent.role(),
                        parse_retries,
                        MAX_PARSE_RETRIES,
                        e
                    );
                    context.add_message(Message::new(agent.role(), &raw));
                    context.add_message(Message::new(
                        AgentRole::Orchestrator,
                        &format!(
                            "[System Error] Your previous output could not be parsed: {}. \
                             Please correct your response format.",
                            e
                        ),
                    ));
                    continue;
                }
            };

            let ParsedOutput { speech, tool_call } = output;

            match tool_call {
                None => {
                    context.clear_volatile_context();
                    return Ok(speech);
                }
                Some(tc) => {
                    if !speech.is_empty() {
                        context.add_message(Message::new(agent.role(), &speech));
                    }

                    tool_calls += 1;
                    if tool_calls > MAX_TOOL_CALLS_PER_TURN {
                        warn!(
                            "Agent {:?} exceeded max tool calls per turn",
                            agent.role()
                        );
                        context.clear_volatile_context();
                        return Ok(format!(
                            "Tool call limit exceeded. Last query: {}",
                            tc.query
                        ));
                    }

                    info!(
                        "Agent {:?} requesting search: {}",
                        agent.role(),
                        tc.query
                    );

                    Self::emit(tx, EngineEvent::ToolCallExecuted {
                        role: agent.role(),
                        action: tc.action.clone(),
                        query: tc.query.clone(),
                    }).await;

                    // Transition to ToolCalling
                    context.transition_to(WorkflowState::ToolCalling {
                        return_to: Box::new(current_state.clone()),
                    })?;

                    let results = tokio::select! {
                        _ = cancel_token.cancelled() => {
                            return Err(AppError::Cancelled);
                        }
                        res = search.search(&tc.query) => res?,
                    };
                    debug!(
                        agent = ?agent.role(),
                        query = tc.query,
                        result_count = results.len(),
                        urls = ?results.iter().map(|r| &r.url).collect::<Vec<_>>(),
                        "Search completed"
                    );
                    let is_fallback_used = results.iter().any(|r| r.is_fallback);
                    let mut formatted = serde_json::to_string_pretty(
                        &results
                            .iter()
                            .map(|r| serde_json::json!({
                                "title": r.title,
                                "url": r.url,
                                "snippet": r.snippet,
                            }))
                            .collect::<Vec<_>>(),
                    )
                    .unwrap_or_else(|_| "[]".to_string());

                    if is_fallback_used {
                        let warning = "[System Warning] This search result is a fallback short snippet. If details are insufficient, refine your search query or proceed with current knowledge.\n\n";
                        formatted = format!("{}{}", warning, formatted);
                    }

                    context.set_volatile_context(formatted);
                    context.transition_to(current_state.clone())?;
                }
            }
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::llm::tests::MockLlmClient;
    use crate::client::search::tests::MockSearchClient;
    use crate::client::search::SearchResult;

    /// Helper: create a channel and return (tx, rx) for engine tests.
    fn event_channel() -> (mpsc::Sender<EngineEvent>, mpsc::Receiver<EngineEvent>) {
        mpsc::channel(128)
    }

    /// Drain all events from the receiver into a Vec.
    async fn collect_events(mut rx: mpsc::Receiver<EngineEvent>) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn happy_path_approve_all() {
        let llm = MockLlmClient::new(vec![
            "Here is the plan: build a REST API".into(),
            "Design: Use actix-web with PostgreSQL".into(),
            "Approve - the design looks solid".into(),
            "Implementation: fn main() { ... }".into(),
            "Approve - LGTM, code matches design".into(),
        ]);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        let _ = engine
            .run(&mut ctx, &llm, &search, "Build a REST API", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions)
            .await
            .unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Completed);
        assert!(ctx.volatile_context().is_none());

        // Verify events
        let events = collect_events(rx).await;
        assert!(events.iter().any(|e| matches!(e, EngineEvent::WorkflowStarted { .. })));
        assert!(events.iter().any(|e| matches!(e, EngineEvent::WorkflowCompleted)));
        // Should have AgentThinking for each agent invocation (5 total)
        let thinking_count = events.iter().filter(|e| matches!(e, EngineEvent::AgentThinking { .. })).count();
        assert_eq!(thinking_count, 5);
        // Should have AgentSpoke for each agent response (5 total)
        let spoke_count = events.iter().filter(|e| matches!(e, EngineEvent::AgentSpoke { .. })).count();
        assert_eq!(spoke_count, 5);
    }

    #[tokio::test]
    async fn search_with_cot_preserved() {
        let llm = MockLlmClient::new(vec![
            "Plan: research and build".into(),
            r#"I need to check the latest version. {"action": "search", "query": "actix-web latest version"}"#.into(),
            "Design: Use actix-web 4.x based on search results".into(),
            "Approve - design is well-researched".into(),
            "impl complete".into(),
            "Approve - all good".into(),
        ]);
        let search = MockSearchClient::new(vec![vec![SearchResult {
            title: "actix-web".into(),
            url: "https://docs.rs/actix-web".into(),
            snippet: "actix-web 4.4.0".into(),
            is_fallback: false,
        }]]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        let _ = engine
            .run(&mut ctx, &llm, &search, "Build with actix", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions)
            .await
            .unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Completed);
        assert_eq!(search.call_count(), 1);
        let messages = ctx.messages();
        assert!(
            messages.iter().any(|m| m.content.contains("I need to check the latest version")),
            "CoT reasoning should be preserved in history"
        );

        // Verify ToolCallExecuted event was emitted
        let events = collect_events(rx).await;
        assert!(events.iter().any(|e| matches!(
            e,
            EngineEvent::ToolCallExecuted { action, query, .. }
            if action == "search" && query == "actix-web latest version"
        )));
    }

    #[tokio::test]
    async fn escalation_on_design_rejection_loop() {
        let mut responses = vec!["Plan: do stuff".to_string()];
        for _ in 0..MAX_ITERATIONS {
            responses.push("Design attempt".into());
            responses.push("Reject - this is terrible".into());
        }
        let llm = MockLlmClient::new(responses);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        let run_future = engine
            .run(&mut ctx, &llm, &search, "Impossible task", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), run_future).await;

        assert_eq!(*ctx.state(), WorkflowState::AwaitingHumanInput);

        let events = collect_events(rx).await;
        assert!(events.iter().any(|e| matches!(e, EngineEvent::WorkflowEscalated { .. })));
    }

    #[tokio::test]
    async fn escalation_on_review_rejection_loop() {
        let mut responses = vec![
            "Plan: build it".to_string(),
            "Design: simple approach".into(),
            "Approve - design is fine".into(),
            "Initial implementation".into(),
        ];
        for _ in 0..MAX_ITERATIONS {
            responses.push("Reject - code has issues".into());
            responses.push("Reworked implementation".into());
        }
        let llm = MockLlmClient::new(responses);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, _rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        let run_future = engine
            .run(&mut ctx, &llm, &search, "Contentious task", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), run_future).await;

        assert_eq!(*ctx.state(), WorkflowState::AwaitingHumanInput);
    }

    #[tokio::test]
    async fn self_correction_on_parse_error() {
        let llm = MockLlmClient::new(vec![
            "Plan: build it".into(),
            "   ".into(),
            "Design: corrected approach".into(),
            "Approve - looks good".into(),
            "Code complete".into(),
            "Approve - ship it".into(),
        ]);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, _rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        engine
            .run(&mut ctx, &llm, &search, "Test self-correction", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions)
            .await
            .unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Completed);
        let messages = ctx.messages();
        assert!(
            messages.iter().any(|m| m.content.contains("[System Error]")),
            "Parse error feedback should be in history"
        );
    }

    #[tokio::test]
    async fn self_correction_escalates_after_max_retries() {
        let llm = MockLlmClient::new(vec![
            "Plan: build it".into(),
            "   ".into(),
            "   ".into(),
            "   ".into(),
            "   ".into(),
        ]);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        let run_future = engine
            .run(&mut ctx, &llm, &search, "Doomed task", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), run_future).await;

        // handle_escalation blocks on the channel, so the state should be AwaitingHumanInput
        assert_eq!(*ctx.state(), WorkflowState::AwaitingHumanInput);

        let events = collect_events(rx).await;
        assert!(events.iter().any(|e| matches!(e, EngineEvent::WorkflowEscalated { .. })));
    }

    #[tokio::test]
    async fn volatile_context_cleared_after_each_phase() {
        let llm = MockLlmClient::new(vec![
            "Plan".into(),
            "Design".into(),
            "Approve".into(),
            "Code".into(),
            "Approve".into(),
        ]);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, _rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        engine
            .run(&mut ctx, &llm, &search, "Test volatile cleanup", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions)
            .await
            .unwrap();

        assert!(ctx.volatile_context().is_none());
    }

    #[tokio::test]
    async fn events_emitted_for_state_transitions() {
        let llm = MockLlmClient::new(vec![
            "Plan".into(),
            "Design".into(),
            "Approve".into(),
            "Code".into(),
            "Approve".into(),
        ]);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();
        let (tx, rx) = event_channel();

        let dummy_interventions = std::sync::Arc::new(dashmap::DashMap::new());
        engine
            .run(&mut ctx, &llm, &search, "Event test", &tx, tokio_util::sync::CancellationToken::new(), dummy_interventions)
            .await
            .unwrap();

        let events = collect_events(rx).await;

        // Verify full event sequence
        let state_changes: Vec<_> = events.iter().filter_map(|e| {
            if let EngineEvent::StateChanged { to, .. } = e {
                Some(to.clone())
            } else {
                None
            }
        }).collect();

        assert_eq!(state_changes, vec![
            WorkflowState::Planning,
            WorkflowState::Designing,
            WorkflowState::Implementing,
            WorkflowState::Reviewing,
            WorkflowState::Completed,
        ]);
    }
}
