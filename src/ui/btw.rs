//! Non-blocking `/btw` side query (dirge-nret).
//!
//! `/btw <question>` fires a one-shot LLM completion that's independent of the
//! session — a quick side conversation. Awaiting it inline in the event loop
//! froze rendering, input, and Ctrl+C for the whole call. This module is the
//! off-thread half: the loop resolves the model on-thread, [`spawn`]s the query
//! as a task that streams the answer back, and a dedicated `select!` arm renders
//! it. Nothing in the session is touched, so it needs no install/continuation
//! machinery — just the answer (or error) to print.

use crate::provider::AnyModel;

/// Handle to the spawned `/btw` task: the result channel the loop drains and the
/// task itself so Ctrl+C can `abort()` it.
pub(crate) struct BtwPhaseHandle {
    pub rx: tokio::sync::mpsc::Receiver<Result<String, String>>,
    pub task: tokio::task::JoinHandle<()>,
}

/// Spawn the `/btw` completion off-thread. `model` is resolved on the UI thread
/// (cheap) and moved into the task, which sends the answer (or a stringified
/// error) back over a capacity-1 channel.
pub(crate) fn spawn(model: AnyModel, query: String) -> BtwPhaseHandle {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, String>>(1);
    let task = tokio::spawn(async move {
        let result = model.btw_query(query).await.map_err(|e| e.to_string());
        let _ = tx.send(result).await;
    });
    BtwPhaseHandle { rx, task }
}
