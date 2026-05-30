//! Plugin-facing LSP query dispatcher.
//!
//! Bridges the Janet `harness/lsp` call (see `plugin::worker`) to the
//! async [`LspManager`]: parse the JSON request the worker forwarded, run
//! the query, and return a JSON-encoded result string. Mirrors the `lsp`
//! tool's operation set and its 1-based line/column convention.
//!
//! Only compiled when both `plugin` and `lsp` are enabled — without
//! `lsp` there's no `LspManager`; without `plugin` there's no caller.

use std::path::Path;

use serde_json::{Value, json};

use crate::lsp::manager::{LspManager, TouchMode};

/// The JSON request shape built by `harness/__lsp` in the plugin worker.
#[derive(serde::Deserialize)]
struct Request {
    op: String,
    file: String,
    #[serde(default)]
    line: u32,
    #[serde(default)]
    char: u32,
    #[serde(default)]
    query: String,
}

/// Run one plugin LSP query and return a JSON string. Errors (bad
/// request, unknown op) are returned as `{"error": "..."}` JSON so the
/// plugin always gets a parseable value.
pub async fn run_query(manager: &LspManager, request_json: &str) -> String {
    let req: Request = match serde_json::from_str(request_json) {
        Ok(r) => r,
        Err(e) => return json!({ "error": format!("invalid lsp request: {e}") }).to_string(),
    };
    let path = Path::new(&req.file);

    // Position/symbol ops need the file in sync with the server first
    // (same as the lsp tool; we don't wait for diagnostics).
    if !matches!(req.op.as_str(), "diagnostics") {
        manager.touch_file(path, TouchMode::Notify).await;
    }

    // 1-based editor coordinates → 0-based LSP wire format.
    let line = req.line.saturating_sub(1);
    let ch = req.char.saturating_sub(1);

    let result: Value = match req.op.as_str() {
        "definition" => json!(manager.definition(path, line, ch).await),
        "references" => json!(manager.references(path, line, ch).await),
        "hover" => json!(manager.hover(path, line, ch).await),
        "implementation" => json!(manager.implementation(path, line, ch).await),
        "documentSymbol" => json!(manager.document_symbol(path).await),
        "workspaceSymbol" => json!(manager.workspace_symbol(path, &req.query).await),
        "prepareCallHierarchy" => json!(manager.prepare_call_hierarchy(path, line, ch).await),
        "incomingCalls" => json!(manager.incoming_calls(path, line, ch).await),
        "outgoingCalls" => json!(manager.outgoing_calls(path, line, ch).await),
        "diagnostics" => {
            // Current published diagnostics for the file. The manager keys
            // by the path it opened the file under; try the literal path
            // then its canonical form.
            let all = manager.all_diagnostics();
            let diags = all
                .get(path)
                .or_else(|| path.canonicalize().ok().and_then(|c| all.get(&c)))
                .cloned()
                .unwrap_or_default();
            json!(diags)
        }
        other => return json!({ "error": format!("unknown lsp op: {other}") }).to_string(),
    };
    result.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::manager::LspManager;
    use crate::lsp::spawn::{Spawned, Spawner};
    use futures::future::BoxFuture;
    use std::sync::Arc;

    /// A spawner that never produces a server. Used for the error-path
    /// tests, where the request is rejected before (bad JSON) or no LSP
    /// server matches the file (unknown op on a bare temp path), so spawn
    /// is never actually invoked.
    struct NoServerSpawner;
    impl Spawner for NoServerSpawner {
        fn spawn<'a>(
            &'a self,
            _server_id: &'a str,
            _root: &'a Path,
        ) -> BoxFuture<'a, std::io::Result<Spawned>> {
            Box::pin(async { Err(std::io::Error::other("no server in test")) })
        }
    }

    fn test_manager() -> LspManager {
        LspManager::new(Arc::new(NoServerSpawner), std::env::temp_dir())
    }

    #[tokio::test]
    async fn invalid_json_returns_error_object() {
        let out = run_query(&test_manager(), "not json at all").await;
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v.get("error")
                .and_then(|e| e.as_str())
                .is_some_and(|s| s.contains("invalid lsp request")),
            "got {out}"
        );
    }

    #[tokio::test]
    async fn unknown_op_returns_error_object() {
        // A file with no matching LSP server → touch_file is a no-op (no
        // spawn), then the unknown op short-circuits to an error.
        let req = json!({ "op": "frobnicate", "file": "/tmp/none.xyz" }).to_string();
        let out = run_query(&test_manager(), &req).await;
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v.get("error").and_then(|e| e.as_str()),
            Some("unknown lsp op: frobnicate"),
            "got {out}"
        );
    }

    #[tokio::test]
    async fn diagnostics_for_untracked_file_is_empty_array() {
        let req = json!({ "op": "diagnostics", "file": "/tmp/never-opened.rs" }).to_string();
        let out = run_query(&test_manager(), &req).await;
        assert_eq!(out, "[]", "got {out}");
    }
}
