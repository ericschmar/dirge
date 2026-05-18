#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manager_new() {
        let mgr = PluginManager::try_new().expect("init must succeed in test env");
        assert!(mgr.hooks.is_empty());
    }

    #[test]
    fn test_try_new_returns_ok() {
        // Construction must be fallible rather than panicking.
        assert!(PluginManager::try_new().is_ok());
    }

    #[test]
    fn test_dispatch_returns_per_hook_results() {
        // Multiple plugins registering the same hook must each contribute
        // a distinct result instead of being silently joined.
        let mut mgr = PluginManager::try_new().unwrap();
        mgr.eval(r#"(defn h1 [ctx] "from-one")"#).unwrap();
        mgr.eval(r#"(defn h2 [ctx] "from-two")"#).unwrap();
        mgr.eval(r#"(defn h-nil [ctx] nil)"#).unwrap();
        mgr.register("on-prompt", "h1");
        mgr.register("on-prompt", "h-nil");
        mgr.register("on-prompt", "h2");

        let out = mgr.dispatch("on-prompt", "@{:prompt \"x\"}").unwrap();
        assert_eq!(out, vec!["from-one".to_string(), "from-two".to_string()]);

        // No hooks registered for this name -> empty vec, still Ok.
        let out = mgr.dispatch("on-error", "@{}").unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn test_take_pending_prompt_returns_literal_nil_string() {
        // A plugin may legitimately request "nil" as a prompt. The
        // harness must distinguish Janet's nil value from a string
        // containing the characters "nil".
        let mut mgr = PluginManager::try_new().unwrap();

        // No pending -> None.
        assert_eq!(mgr.take_pending_prompt(), None);

        // Literal string "nil" must round-trip.
        mgr.eval(r#"(harness/request-prompt "nil")"#).unwrap();
        assert_eq!(mgr.take_pending_prompt(), Some("nil".to_string()));

        // After take, slot is cleared.
        assert_eq!(mgr.take_pending_prompt(), None);

        // Non-string requests are rejected by the harness.
        mgr.eval(r#"(harness/request-prompt 42)"#).unwrap();
        assert_eq!(mgr.take_pending_prompt(), None);
    }

    #[test]
    fn test_post_done_action() {
        // Plugin followup must take precedence over the loop iteration
        // so we never silently drop a queued prompt.
        let followup = Some("retry".to_string());
        assert_eq!(
            decide_post_done_action(followup.clone(), true, false),
            PostDoneAction::Followup("retry".into())
        );
        assert_eq!(
            decide_post_done_action(followup.clone(), false, false),
            PostDoneAction::Followup("retry".into())
        );
        // Loop iteration only when no followup.
        assert_eq!(
            decide_post_done_action(None, true, false),
            PostDoneAction::LoopIter
        );
        // Loop stop only when no followup and should_stop.
        assert_eq!(
            decide_post_done_action(None, true, true),
            PostDoneAction::LoopStop
        );
        // Idle: nothing to do.
        assert_eq!(
            decide_post_done_action(None, false, false),
            PostDoneAction::Idle
        );
    }

    #[test]
    fn test_poisoned_mutex_recovery_pattern() {
        // PluginManager owns a JanetClient which is !Send, so we can't
        // poison it across threads directly. Verify the recovery
        // pattern itself: `unwrap_or_else(|e| e.into_inner())` must
        // still hand us the inner value after a thread panic.
        use std::sync::{Arc, Mutex};
        let m: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _guard = m2.lock().unwrap();
            panic!("intentional poison");
        })
        .join();

        assert!(m.is_poisoned(), "thread panic must poison the mutex");
        let mut guard = m.lock().unwrap_or_else(|e| e.into_inner());
        guard.push("ok".to_string());
        assert_eq!(guard.as_slice(), &["ok".to_string()]);
    }

    #[test]
    fn test_filter_existing_dirs() {
        use std::path::PathBuf;
        let tmp = std::env::temp_dir().join(format!("dirge-plugin-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let exists = tmp.clone();
        let missing = tmp.join("does-not-exist");
        let kept = filter_existing_dirs(&[exists.clone(), missing.clone()]);
        assert_eq!(kept, vec![exists.clone()]);
        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
        // Empty input -> empty output
        let none: Vec<PathBuf> = filter_existing_dirs(&[]);
        assert!(none.is_empty());
    }

    #[test]
    fn test_register_hook() {
        let mut mgr = PluginManager::try_new().unwrap();
        mgr.register("on-init", "test-init");
        assert_eq!(mgr.hooks.len(), 1);
        assert!(mgr.hooks.contains_key("on-init"));
    }

    #[test]
    fn test_register_multiple_hooks() {
        let mut mgr = PluginManager::try_new().unwrap();
        mgr.register("on-init", "test-init");
        mgr.register("on-prompt", "test-prompt");
        mgr.register("on-response", "test-response");
        assert_eq!(mgr.hooks.len(), 3);
    }

    #[test]
    fn test_load_and_eval_janet() {
        let mut mgr = PluginManager::try_new().unwrap();
        let result = mgr.eval("(+ 1 2)");
        assert_eq!(result, Ok("3".to_string()));
    }

    #[test]
    fn test_load_and_eval_janet_error() {
        let mut mgr = PluginManager::try_new().unwrap();
        let result = mgr.eval("(undefined-fn 1)");
        assert!(result.is_err());
    }

    #[test]
    fn test_dispatch_hook() {
        let mut mgr = PluginManager::try_new().unwrap();
        mgr.eval("(defn on-init [ctx] (string \"loaded with model: \" (ctx :model)))")
            .unwrap();
        mgr.register("on-init", "on-init");
        let result = mgr.dispatch("on-init", "@{:model \"gpt-4\"}").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("loaded with model: gpt-4"));
    }

    #[test]
    fn test_harness_log() {
        let mut mgr = PluginManager::try_new().unwrap();
        let result = mgr.eval("(harness/log \"hello from plugin\")");
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_file() {
        let mut mgr = PluginManager::try_new().unwrap();
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join("test_plugin.janet");
        mgr.load_file(&fixtures).unwrap();
        mgr.register("on-init", "on-init");
        let result = mgr.dispatch("on-init", "@{:model \"test\"}").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("loaded with test"));
    }

    #[test]
    fn test_auto_discover_hooks() {
        let mut mgr = PluginManager::try_new().unwrap();
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join("test_plugin.janet");
        mgr.load_file(&fixtures).unwrap();

        // Simulate auto-discovery: check each hook and register if found.
        // Use has_symbol so missing hooks don't trigger Janet's
        // "unknown symbol" stderr noise.
        let hook_names = [
            "on-init",
            "on-prompt",
            "on-response",
            "on-tool-start",
            "on-tool-end",
            "on-error",
            "on-complete",
        ];
        let mut found = 0;
        for hook in &hook_names {
            if mgr.has_symbol(hook) {
                mgr.register(hook, hook);
                found += 1;
            }
        }
        assert_eq!(found, 3, "should find on-init, on-prompt, on-response");

        // Symbols that aren't defined must report false.
        assert!(!mgr.has_symbol("on-tool-start"));
        assert!(!mgr.has_symbol("totally-unknown-fn"));

        // on-init
        let r = mgr.dispatch("on-init", "@{:model \"test\"}").unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].contains("loaded with test"));

        // on-prompt with matching text
        assert_eq!(
            mgr.dispatch("on-prompt", "@{:prompt \"hello world\"}")
                .unwrap(),
            vec!["greeting detected".to_string()]
        );

        // on-prompt with non-matching text (hook returns nil -> empty Vec)
        assert!(
            mgr.dispatch("on-prompt", "@{:prompt \"goodbye\"}")
                .unwrap()
                .is_empty()
        );

        // on-response with matching text
        assert_eq!(
            mgr.dispatch("on-response", "@{:response \"error: panic\"}")
                .unwrap(),
            vec!["error in response".to_string()]
        );

        // unknown hook returns empty
        assert!(
            mgr.dispatch("on-tool-start", "@{:tool \"bash\"}")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_janet_escaping() {
        let mut mgr = PluginManager::try_new().unwrap();

        // Define a test function
        mgr.eval(r#"(defn test-echo [ctx] (ctx :msg))"#).unwrap();
        mgr.register("on-prompt", "test-echo");

        // Quotes in text
        assert_eq!(
            mgr.dispatch("on-prompt", "@{:msg \"he said \\\"hello\\\"\"}")
                .unwrap(),
            vec!["he said \"hello\"".to_string()]
        );

        // Backslashes in text
        assert_eq!(
            mgr.dispatch("on-prompt", "@{:msg \"path\\\\to\\\\file\"}")
                .unwrap(),
            vec!["path\\to\\file".to_string()]
        );

        // Newlines in text
        assert_eq!(
            mgr.dispatch("on-prompt", "@{:msg \"line1\\nline2\"}")
                .unwrap(),
            vec!["line1\nline2".to_string()]
        );
    }

    #[test]
    fn test_escape_janet_string() {
        assert_eq!(escape_janet_string("simple"), "simple");
        assert_eq!(escape_janet_string("a\"b"), "a\\\"b");
        assert_eq!(escape_janet_string("a\\b"), "a\\\\b");
        assert_eq!(escape_janet_string("a\nb\tc\rd"), "a\\nb\\tc\\rd");
        // control char -> hex escape
        assert_eq!(escape_janet_string("a\x01b"), "a\\x01b");
    }

    #[test]
    fn test_dispatch_swallows_runtime_errors() {
        // A misbehaving plugin should not crash dispatch or pollute output.
        let mut mgr = PluginManager::try_new().unwrap();
        mgr.eval(r#"(defn broken [ctx] (string/find "x" nil))"#)
            .unwrap();
        mgr.register("on-prompt", "broken");
        let result = mgr.dispatch("on-prompt", "@{:prompt \"hi\"}").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_dispatch_with_json_args_as_string() {
        // Tool args arrive as JSON; the harness escapes them into a
        // Janet string so the parser never has to handle {":", ","}.
        let mut mgr = PluginManager::try_new().unwrap();
        mgr.eval(r#"(defn capture [ctx] (ctx :args))"#).unwrap();
        mgr.register("on-tool-start", "capture");
        let args_json = r#"{"path": "/tmp/x", "n": null, "xs": [1, 2, 3]}"#;
        let ctx = format!(
            "@{{:tool \"Bash\" :args \"{}\"}}",
            escape_janet_string(args_json)
        );
        let result = mgr.dispatch("on-tool-start", &ctx).unwrap();
        assert_eq!(result, vec![args_json.to_string()]);
    }

    #[test]
    fn test_has_symbol() {
        let mut mgr = PluginManager::try_new().unwrap();
        mgr.eval("(defn my-hook [ctx] :ok)").unwrap();
        assert!(mgr.has_symbol("my-hook"));
        assert!(!mgr.has_symbol("nope-not-here"));
        // weird names with hyphens/quotes shouldn't crash
        assert!(!mgr.has_symbol("a\"b-c"));
    }

    #[test]
    fn test_janet_phase_tracking() {
        let mut mgr = PluginManager::try_new().unwrap();

        // Define test functions that use harness APIs
        mgr.eval(
            r#"
            (var test-phase :idle)
            (defn test-on-init [ctx]
              (harness/log "phase test loaded")
              nil)
            (defn test-on-prompt [ctx]
              (case test-phase
                :idle (do (set test-phase :active) "entered active")
                :active (do (set test-phase :done) "entered done")
                nil))
        "#,
        )
        .unwrap();

        mgr.register("on-init", "test-on-init");
        mgr.register("on-prompt", "test-on-prompt");

        // on-init should work
        let result = mgr.dispatch("on-init", "@{}");
        assert!(result.is_ok());

        // First prompt: idle -> active
        assert_eq!(
            mgr.dispatch("on-prompt", "@{:prompt \"any\"}").unwrap(),
            vec!["entered active".to_string()]
        );

        // Second prompt: active -> done
        assert_eq!(
            mgr.dispatch("on-prompt", "@{:prompt \"any\"}").unwrap(),
            vec!["entered done".to_string()]
        );

        // Third prompt: done -> nil -> empty
        assert!(
            mgr.dispatch("on-prompt", "@{:prompt \"any\"}")
                .unwrap()
                .is_empty()
        );
    }
}

use std::collections::HashMap;

#[cfg(feature = "plugin")]
use janetrs::client::{Error as JanetError, JanetClient};

/// Escape a Rust string so it can be safely embedded inside a Janet
/// double-quoted string literal. Janet's parser accepts the standard
/// `\"`, `\\`, `\n`, `\r`, `\t` escapes, so we normalise all of those
/// plus any remaining ASCII control characters via `\xNN`.
#[cfg_attr(not(feature = "plugin"), allow(dead_code))]
pub fn escape_janet_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\x{:02X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// What the host should do after an agent turn completes.
/// Plugin followups must outrank loop iterations so a queued
/// `harness/request-prompt` never gets silently overwritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostDoneAction {
    Followup(String),
    LoopIter,
    LoopStop,
    Idle,
}

pub fn decide_post_done_action(
    followup: Option<String>,
    loop_active: bool,
    loop_should_stop: bool,
) -> PostDoneAction {
    if let Some(text) = followup {
        return PostDoneAction::Followup(text);
    }
    if !loop_active {
        return PostDoneAction::Idle;
    }
    if loop_should_stop {
        PostDoneAction::LoopStop
    } else {
        PostDoneAction::LoopIter
    }
}

/// Filter a list of candidate plugin dirs down to those that exist.
/// Used at startup to silently skip default search paths that aren't
/// present rather than spamming "plugin dir not found" warnings.
#[cfg_attr(not(feature = "plugin"), allow(dead_code))]
pub fn filter_existing_dirs(candidates: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    candidates.iter().filter(|p| p.is_dir()).cloned().collect()
}

pub struct PluginManager {
    hooks: HashMap<String, Vec<String>>,
    #[cfg(feature = "plugin")]
    client: JanetClient,
}

#[cfg_attr(not(feature = "plugin"), allow(dead_code))]
impl PluginManager {
    /// Initialize a Janet VM and the harness API. Returns Err if Janet
    /// init fails (e.g. already initialized on this thread) so the host
    /// can fall back instead of panicking.
    pub fn try_new() -> Result<Self, String> {
        #[cfg(feature = "plugin")]
        let client = {
            let c = JanetClient::init_with_default_env()
                .map_err(|e| format!("Failed to initialize Janet VM: {e}"))?;

            // Define harness API functions in Janet
            let _ = c.run(
                r#"
                (var harness-pending nil)
                (var harness-response nil)

                (defn harness/log [msg] (print "[plugin] " msg))
                (defn harness/get-cwd [] (os/cwd))
                (defn harness/request-prompt [prompt]
                  (when (string? prompt)
                    (set harness-pending prompt)))
                (defn harness/store-response [resp]
                  (set harness-response resp))
                (defn harness/has-symbol? [name]
                  (truthy? (get (curenv) (symbol name))))
            "#,
            );

            c
        };

        Ok(PluginManager {
            hooks: HashMap::new(),
            #[cfg(feature = "plugin")]
            client,
        })
    }

    #[cfg(feature = "plugin")]
    pub fn load_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("Failed to read plugin: {e}"))?;
        self.eval(&content)?;
        Ok(())
    }

    #[cfg(not(feature = "plugin"))]
    pub fn load_file(&mut self, _path: &std::path::Path) -> Result<(), String> {
        Ok(())
    }

    pub fn register(&mut self, hook: &str, script: &str) {
        self.hooks
            .entry(hook.to_string())
            .or_default()
            .push(script.to_string());
    }

    #[cfg(feature = "plugin")]
    pub fn take_pending_prompt(&mut self) -> Option<String> {
        // Stringify on the Janet side so we can disambiguate Janet's
        // nil value from a string with the characters "nil". Probe the
        // type first; only fetch the value if it really is a string.
        let is_string = self
            .client
            .run("(if (string? harness-pending) true false)")
            .map(|v| v.to_string() == "true")
            .unwrap_or(false);
        if !is_string {
            return None;
        }
        let val = match self.client.run("harness-pending") {
            Ok(v) => v,
            Err(_) => return None,
        };
        let s = val.to_string();
        let _ = self.client.run("(set harness-pending nil)");
        Some(s)
    }

    #[cfg(not(feature = "plugin"))]
    pub fn take_pending_prompt(&mut self) -> Option<String> {
        None
    }

    #[cfg(feature = "plugin")]
    pub fn store_response(&mut self, response: &str) {
        let escaped = escape_janet_string(response);
        let _ = self
            .client
            .run(&format!(r#"(set harness-response "{}")"#, escaped));
    }

    #[cfg(not(feature = "plugin"))]
    pub fn store_response(&mut self, _response: &str) {}

    /// Check whether a top-level symbol is bound in the Janet env
    /// without triggering Janet's compile-error stderr output.
    #[cfg(feature = "plugin")]
    pub fn has_symbol(&mut self, name: &str) -> bool {
        let escaped = escape_janet_string(name);
        let code = format!(r#"(harness/has-symbol? "{}")"#, escaped);
        match self.client.run(&code) {
            Ok(val) => val.to_string() == "true",
            Err(_) => false,
        }
    }

    #[cfg(not(feature = "plugin"))]
    pub fn has_symbol(&mut self, _name: &str) -> bool {
        false
    }

    #[cfg(feature = "plugin")]
    pub fn eval(&mut self, code: &str) -> Result<String, String> {
        self.client
            .run(code)
            .map(|val| val.to_string())
            .map_err(|e: JanetError| format!("Janet error: {e}"))
    }

    #[cfg(not(feature = "plugin"))]
    pub fn eval(&mut self, _code: &str) -> Result<String, String> {
        Err("plugin feature not enabled".to_string())
    }

    #[cfg(feature = "plugin")]
    pub fn dispatch(&mut self, hook: &str, context_janet: &str) -> Result<Vec<String>, String> {
        let names = match self.hooks.get(hook) {
            Some(names) => names.clone(),
            None => return Ok(Vec::new()),
        };

        let mut results = Vec::new();
        for name in &names {
            // Wrap the call in (try ... ([err] nil)) so plugin runtime
            // errors don't print Janet stack traces to stderr.
            let code = format!(
                r#"(try (do (def ctx {ctx}) ({fname} ctx)) ([err fib] nil))"#,
                ctx = context_janet,
                fname = name,
            );
            if let Ok(result) = self.eval(&code) {
                let s = result.to_string();
                // Janet nil -> skip
                if s != "nil" && !s.is_empty() {
                    results.push(s);
                }
            }
        }

        Ok(results)
    }

    #[cfg(not(feature = "plugin"))]
    pub fn dispatch(&mut self, _hook: &str, _context_janet: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}
