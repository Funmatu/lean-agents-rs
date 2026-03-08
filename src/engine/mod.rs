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
use crate::domain::message::Message;
use crate::domain::state::WorkflowState;
use crate::error::AppError;
use crate::parser::{self, AgentOutput};

const MAX_ITERATIONS: u32 = 3;
const MAX_TOOL_CALLS_PER_TURN: u32 = 3;

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

    /// Run the full workflow engine loop.
    pub async fn run(
        &self,
        context: &mut ContextGraph,
        llm: &dyn LlmClient,
        search: &dyn SearchClient,
        task: &str,
    ) -> Result<(), AppError> {
        // Init -> Planning
        context.transition_to(WorkflowState::Planning)?;
        context.add_message(Message::new(AgentRole::Orchestrator, task));
        info!("Engine started: Init -> Planning");

        // Planning phase: Orchestrator creates a plan
        let plan = self.execute_with_tool_support(
            &self.orchestrator,
            context,
            llm,
            search,
        ).await?;
        context.add_message(Message::new(AgentRole::Orchestrator, &plan));

        // Planning -> Designing
        context.transition_to(WorkflowState::Designing)?;
        info!("Planning -> Designing");

        // Design loop: Architect proposes, DevilsAdvocate reviews
        let mut design_iterations = 0u32;
        loop {
            if design_iterations >= MAX_ITERATIONS {
                warn!("Design phase exceeded MAX_ITERATIONS, escalating");
                context.transition_to(WorkflowState::Escalated)?;
                return Ok(());
            }

            // Architect designs
            let design = self.execute_with_tool_support(
                &self.architect,
                context,
                llm,
                search,
            ).await?;
            context.add_message(Message::new(AgentRole::Architect, &design));

            // DevilsAdvocate reviews design
            let review = self.execute_with_tool_support(
                &self.devils_advocate,
                context,
                llm,
                search,
            ).await?;
            context.add_message(Message::new(AgentRole::DevilsAdvocate, &review));

            if parser::is_approval(&review) {
                info!("Design approved by DevilsAdvocate");
                break;
            }

            design_iterations += 1;
            info!("Design iteration {design_iterations}: revision requested");
        }

        // Designing -> Implementing
        context.transition_to(WorkflowState::Implementing)?;
        info!("Designing -> Implementing");

        // Programmer implements
        let implementation = self.execute_with_tool_support(
            &self.programmer,
            context,
            llm,
            search,
        ).await?;
        context.add_message(Message::new(AgentRole::Programmer, &implementation));

        // Implementing -> Reviewing
        context.transition_to(WorkflowState::Reviewing)?;
        info!("Implementing -> Reviewing");

        // Review loop: DevilsAdvocate reviews, may send back to Programmer
        let mut review_iterations = 0u32;
        loop {
            if review_iterations >= MAX_ITERATIONS {
                warn!("Review phase exceeded MAX_ITERATIONS, escalating");
                context.transition_to(WorkflowState::Escalated)?;
                return Ok(());
            }

            let review = self.execute_with_tool_support(
                &self.devils_advocate,
                context,
                llm,
                search,
            ).await?;
            context.add_message(Message::new(AgentRole::DevilsAdvocate, &review));

            if parser::is_approval(&review) {
                info!("Implementation approved by DevilsAdvocate");
                break;
            }

            // Rework: Reviewing -> Implementing -> Reviewing
            context.transition_to(WorkflowState::Implementing)?;
            let rework = self.execute_with_tool_support(
                &self.programmer,
                context,
                llm,
                search,
            ).await?;
            context.add_message(Message::new(AgentRole::Programmer, &rework));
            context.transition_to(WorkflowState::Reviewing)?;

            review_iterations += 1;
            info!("Review iteration {review_iterations}: rework requested");
        }

        // Reviewing -> Completed
        context.transition_to(WorkflowState::Completed)?;
        info!("Workflow completed successfully");

        Ok(())
    }

    /// Execute an agent with tool call interception.
    /// If the agent outputs a tool call, perform the search, store result
    /// in volatile_context, and re-invoke the agent.
    async fn execute_with_tool_support(
        &self,
        agent: &dyn Agent,
        context: &mut ContextGraph,
        llm: &dyn LlmClient,
        search: &dyn SearchClient,
    ) -> Result<String, AppError> {
        let current_state = context.state().clone();
        let mut tool_calls = 0u32;

        loop {
            let raw = agent.execute(context, llm).await?;
            let output = parser::parse_agent_output(&raw)?;

            match output {
                AgentOutput::Speech(text) => {
                    // Clear volatile context after final answer
                    context.clear_volatile_context();
                    return Ok(text);
                }
                AgentOutput::ToolCall(tc) => {
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

                    // Transition to ToolCalling
                    context.transition_to(WorkflowState::ToolCalling {
                        return_to: Box::new(current_state.clone()),
                    })?;

                    // Perform search and format as structured JSON to prevent prompt injection
                    let results = search.search(&tc.query).await?;
                    debug!(
                        agent = ?agent.role(),
                        query = tc.query,
                        result_count = results.len(),
                        urls = ?results.iter().map(|r| &r.url).collect::<Vec<_>>(),
                        "Search completed"
                    );
                    let formatted = serde_json::to_string_pretty(
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

                    // Store in volatile context
                    context.set_volatile_context(formatted);

                    // Return to original state
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

    #[tokio::test]
    async fn happy_path_approve_all() {
        // Mock LLM responses for the full workflow:
        // 1. Orchestrator plan
        // 2. Architect design
        // 3. DevilsAdvocate approves design
        // 4. Programmer implementation
        // 5. DevilsAdvocate approves implementation
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

        engine
            .run(&mut ctx, &llm, &search, "Build a REST API")
            .await
            .unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Completed);
        assert!(ctx.volatile_context().is_none());
    }

    #[tokio::test]
    async fn search_then_approve_path() {
        // Architect requests a search, then approves after getting results
        let llm = MockLlmClient::new(vec![
            "Plan: research and build".into(),
            // Architect's first response is a tool call
            r#"{"action": "search", "query": "actix-web latest version"}"#.into(),
            // After getting search results, Architect gives design
            "Design: Use actix-web 4.x based on search results".into(),
            "Approve - design is well-researched".into(),
            "impl complete".into(),
            "Approve - all good".into(),
        ]);
        let search = MockSearchClient::new(vec![vec![SearchResult {
            title: "actix-web".into(),
            url: "https://docs.rs/actix-web".into(),
            snippet: "actix-web 4.4.0".into(),
        }]]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();

        engine
            .run(&mut ctx, &llm, &search, "Build with actix")
            .await
            .unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Completed);
        assert_eq!(search.call_count(), 1);
    }

    #[tokio::test]
    async fn escalation_on_design_rejection_loop() {
        // DevilsAdvocate always rejects -> should escalate after MAX_ITERATIONS
        let mut responses = vec!["Plan: do stuff".to_string()];
        for _ in 0..MAX_ITERATIONS {
            responses.push("Design attempt".into());
            responses.push("Reject - this is terrible".into());
        }
        let llm = MockLlmClient::new(responses);
        let search = MockSearchClient::new(vec![]);
        let mut ctx = ContextGraph::new();
        let engine = Engine::new();

        engine
            .run(&mut ctx, &llm, &search, "Impossible task")
            .await
            .unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Escalated);
    }

    #[tokio::test]
    async fn escalation_on_review_rejection_loop() {
        // Design approved, but review always rejects
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

        engine
            .run(&mut ctx, &llm, &search, "Contentious task")
            .await
            .unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Escalated);
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

        engine
            .run(&mut ctx, &llm, &search, "Test volatile cleanup")
            .await
            .unwrap();

        // volatile_context should always be None after completion
        assert!(ctx.volatile_context().is_none());
    }
}
