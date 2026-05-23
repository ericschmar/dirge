use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agent::tools::background::{BackgroundStore, TaskState};
use crate::agent::tools::{AskSender, PermCheck, ToolError, check_perm};
use crate::provider::AnyModel;

/// dirge-ov2 Phase D: subagent chat-window event. Sent by `TaskTool`
/// when it spawns / completes a subagent so the UI loop can surface
/// the subagent's lifecycle as a chat-window (Ctrl-N/P/X to switch
/// to it via the multi-chat infrastructure landed in Phases A-C).
///
/// `id` is the subagent's task id (UUID for background tasks; a
/// freshly-generated UUID for foreground tasks). The UI loop keys
/// chat windows on this id so multiple concurrent subagents get
/// distinct windows.
///
/// First-pass design: prompt + final result are emitted; per-token
/// streaming isn't wired through. A follow-up will route the full
/// agent-loop event stream once `TaskTool` migrates from `btw_query`
/// (one-shot, tool-less) to a proper sub-runner with the parent's
/// tool set. Phase A-C laid the multi-chat infrastructure that
/// rewrite needs; Phase D ships visibility today.
#[derive(Debug, Clone)]
pub enum SubagentChatEvent {
    /// A new subagent is starting. UI loop creates a chat window
    /// named after a short truncation of the prompt and writes the
    /// prompt as the first line.
    Spawn { id: String, prompt: String },
    /// Subagent finished successfully. UI loop writes `result` to
    /// the matching chat window.
    Complete { id: String, result: String },
    /// Subagent errored or timed out. UI loop writes the failure
    /// reason in error color.
    Failed { id: String, error: String },
}

pub type SubagentChatSender = mpsc::UnboundedSender<SubagentChatEvent>;

/// Receiver side of the subagent chat-event channel — exposed for
/// the UI loop's `tokio::select!` arm. Only consumed in main.rs +
/// ui/mod.rs; marked `dead_code`-allow because the producer side
/// (TaskTool) lives in this module and `cargo check` sees only the
/// definition site, not the cross-module consumer.
#[allow(dead_code)]
pub type SubagentChatReceiver = mpsc::UnboundedReceiver<SubagentChatEvent>;

/// dirge-ov2 Phase D: process-global sender for subagent chat
/// events. Set once at interactive-session startup; every TaskTool
/// reads it lazily so the builder doesn't need to thread the
/// channel through 13 call sites.
///
/// A follow-up could replace this with proper threading through
/// `BuilderContext` — for now the global keeps the Phase D diff
/// small and the test path (no global set) behaves like pre-ov2.
static SUBAGENT_CHAT_SINK: std::sync::OnceLock<SubagentChatSender> = std::sync::OnceLock::new();

pub fn set_subagent_chat_sink(sink: SubagentChatSender) {
    // OnceLock — first writer wins. Re-set is a no-op (logged via
    // tracing for visibility but not fatal because tests / hot
    // reload may try to set twice).
    if SUBAGENT_CHAT_SINK.set(sink).is_err() {
        tracing::debug!("subagent chat sink already set; ignoring re-set");
    }
}

pub fn subagent_chat_sink() -> Option<SubagentChatSender> {
    SUBAGENT_CHAT_SINK.get().cloned()
}

pub struct TaskTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    model: AnyModel,
    bg_store: BackgroundStore,
    /// dirge-ov2: send-side of the subagent-chat-event channel.
    /// `Option` so `--no-tools` paths / tests can omit the UI sink
    /// without forcing every TaskTool builder to manufacture one.
    chat_sink: Option<SubagentChatSender>,
}

impl TaskTool {
    pub fn new(
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        model: AnyModel,
        bg_store: BackgroundStore,
    ) -> Self {
        Self {
            permission,
            ask_tx,
            model,
            bg_store,
            chat_sink: None,
        }
    }

    /// dirge-ov2: wire the subagent-chat-event sender. Called by the
    /// agent builder when constructing the TaskTool for an
    /// interactive session. Headless / test paths skip this so the
    /// tool behaves identically to the pre-ov2 implementation.
    ///
    /// Currently unused in production — the process-global sink
    /// (set via `set_subagent_chat_sink`) is the wired path. Kept
    /// for tests + future per-instance overrides.
    #[allow(dead_code)]
    pub fn with_chat_sink(mut self, sink: SubagentChatSender) -> Self {
        self.chat_sink = Some(sink);
        self
    }

    /// dirge-ov2 helper: fire-and-forget a chat event. Prefers the
    /// instance-bound sink (set via `with_chat_sink`); falls back
    /// to the process-global sink set at UI-loop startup. If
    /// neither is installed (headless / tests) the event is
    /// silently discarded — never block the subagent or fail the
    /// tool call on UI plumbing trouble.
    fn emit_chat(&self, event: SubagentChatEvent) {
        if let Some(sink) = &self.chat_sink {
            let _ = sink.send(event);
            return;
        }
        if let Some(sink) = subagent_chat_sink() {
            let _ = sink.send(event);
        }
    }
}

#[derive(Deserialize)]
pub struct Args {
    pub prompt: String,
    #[serde(default)]
    pub background: Option<bool>,
}

impl Tool for TaskTool {
    const NAME: &'static str = "task";

    type Error = ToolError;
    type Args = Args;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let description = "Spawn a subagent to handle a specific subtask. The subagent runs as a one-shot query (no tools) and returns its result inline. Use for research, analysis, or planning subtasks that don't require file access. Set background=true to run asynchronously — completion is delivered to you automatically as a <system-reminder> at the start of your next turn. Do NOT poll task_status in a loop or sleep waiting; continue with other work."
            .to_string();

        let properties = serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Task description for the subagent"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run asynchronously (default: false). When true, returns a task_id immediately. The result is delivered automatically as a <system-reminder> on your next turn — do NOT poll task_status."
                }
            },
            "required": ["prompt"]
        });

        ToolDefinition {
            name: "task".to_string(),
            description,
            parameters: properties,
        }
    }

    async fn call(&self, args: Args) -> Result<String, ToolError> {
        check_perm(&self.permission, &self.ask_tx, "task", &args.prompt).await?;

        let background = args.background.unwrap_or(false);

        if background {
            // Audit M2: refuse new background spawns past the
            // concurrency cap. The agent gets a clear refusal it
            // can act on (wait for an existing task to finish, then
            // retry) rather than fanning out unbounded.
            let running = self.bg_store.running_count();
            let cap = BackgroundStore::max_concurrent();
            if running >= cap {
                return Err(ToolError::Msg(format!(
                    "background subagent cap reached ({}/{} in flight). Wait for one to finish (use task_status) or run inline (background=false). Capping prevents fan-out from burning the API budget.",
                    running, cap,
                )));
            }
            let task_id = Uuid::new_v4().to_string();
            self.bg_store.insert(task_id.clone());
            self.bg_store.notify_started(&task_id);

            // dirge-ov2 Phase D: announce the subagent so the UI
            // loop creates a chat window for it.
            self.emit_chat(SubagentChatEvent::Spawn {
                id: task_id.clone(),
                prompt: args.prompt.clone(),
            });

            let model = self.model.clone();
            let prompt = args.prompt;
            let store = self.bg_store.clone();
            let tid = task_id.clone();
            let chat_sink = self.chat_sink.clone();

            // Cap the background subagent at 10 minutes. Without a
            // timeout, a stuck subagent (provider hang, runaway
            // multi-turn) would keep the task in `Running` state
            // forever, hold its model/network handle open, and
            // never deliver a system-reminder to the next turn.
            // 10 min matches the rough upper bound for a coherent
            // single-prompt LLM task; anything longer is the
            // subagent loop misbehaving.
            const SUBAGENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
            let store_for_task = store.clone();
            let tid_for_task = tid.clone();
            let handle = tokio::spawn(async move {
                let fut = model.btw_query(format!(
                    "You are a subagent working on a specific subtask. Complete it thoroughly.\n\nTask: {}",
                    prompt
                ));
                let result = tokio::time::timeout(SUBAGENT_TIMEOUT, fut).await;
                let (state, chat_event) = match result {
                    Ok(Ok(text)) => (
                        TaskState::Completed(text.clone()),
                        SubagentChatEvent::Complete {
                            id: tid_for_task.clone(),
                            result: text,
                        },
                    ),
                    Ok(Err(e)) => {
                        let msg = e.to_string();
                        (
                            TaskState::Failed(msg.clone()),
                            SubagentChatEvent::Failed {
                                id: tid_for_task.clone(),
                                error: msg,
                            },
                        )
                    }
                    Err(_) => {
                        let msg =
                            format!("subagent timed out after {}s", SUBAGENT_TIMEOUT.as_secs(),);
                        (
                            TaskState::Failed(msg.clone()),
                            SubagentChatEvent::Failed {
                                id: tid_for_task.clone(),
                                error: msg,
                            },
                        )
                    }
                };
                if let Some(sink) = chat_sink {
                    let _ = sink.send(chat_event);
                }
                store_for_task.notify(&tid_for_task, state);
            });
            // Register the handle so `BackgroundStore::cancel_all` (called
            // on session swap) can abort the subagent and free its
            // provider connection. Without this the task survived the
            // parent's session change and kept consuming API budget.
            store.attach_handle(&tid, handle);

            Ok(format!(
                "background task started — task_id: {}\n\nThe subagent runs in the background. Completion will be delivered automatically as a <system-reminder> at the start of your next turn. Do NOT poll task_status or sleep waiting — continue with other work.",
                task_id
            ))
        } else {
            // dirge-ov2 Phase D: foreground subagent. Emit Spawn /
            // Complete / Failed so the UI surfaces the call as a
            // chat window (Ctrl-N/P/X to view). Foreground tasks
            // still block the parent agent's tool call; the chat
            // window populates with prompt + final result.
            let task_id = Uuid::new_v4().to_string();
            self.emit_chat(SubagentChatEvent::Spawn {
                id: task_id.clone(),
                prompt: args.prompt.clone(),
            });
            let result = self
                .model
                .btw_query(format!(
                    "You are a subagent working on a specific subtask. Complete it thoroughly.\n\nTask: {}",
                    args.prompt
                ))
                .await;
            match result {
                Ok(text) => {
                    self.emit_chat(SubagentChatEvent::Complete {
                        id: task_id,
                        result: text.clone(),
                    });
                    Ok(text)
                }
                Err(e) => {
                    let msg = e.to_string();
                    self.emit_chat(SubagentChatEvent::Failed {
                        id: task_id,
                        error: msg.clone(),
                    });
                    Err(ToolError::Msg(format!("Subagent error: {}", msg)))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::background::BackgroundStore;
    use crate::provider::AnyModel;
    use rig::client::CompletionClient;
    use rig::providers::openrouter;

    fn mock_tool() -> TaskTool {
        // The model is never invoked in these tests — they exercise the
        // definition surface only.
        let client = openrouter::Client::new("test-key").unwrap();
        let model = client.completion_model("anthropic/claude-sonnet-4.5");
        TaskTool::new(
            None,
            None,
            AnyModel::OpenRouter(model),
            BackgroundStore::new(),
        )
    }

    // Regression: the task tool description must tell the agent that
    // background=true delivers completion automatically and instruct it
    // NOT to poll task_status. The previous text told the agent to "use
    // task_status to poll for the result", which produced wasteful loops.
    #[tokio::test]
    async fn definition_steers_agent_away_from_polling() {
        let tool = mock_tool();
        let def = tool.definition(String::new()).await;
        let desc = def.description.to_lowercase();
        assert!(
            desc.contains("system-reminder") || desc.contains("automatically"),
            "task description must reference automatic notification: {}",
            def.description
        );
        assert!(
            desc.contains("do not poll") || desc.contains("not poll"),
            "task description must explicitly discourage polling: {}",
            def.description
        );
    }

    #[tokio::test]
    async fn definition_advertises_background_field() {
        let tool = mock_tool();
        let def = tool.definition(String::new()).await;
        let props = def
            .parameters
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("properties present");
        assert!(props.contains_key("background"));
        let bg_desc = props["background"]["description"]
            .as_str()
            .unwrap()
            .to_lowercase();
        assert!(bg_desc.contains("automatically") || bg_desc.contains("system-reminder"));
        assert!(bg_desc.contains("do not poll") || bg_desc.contains("not poll"));
    }
}
