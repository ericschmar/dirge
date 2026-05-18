#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manager_new() {
        let mgr = PluginManager::new();
        assert!(mgr.hooks.is_empty());
    }

    #[test]
    fn test_register_hook() {
        let mut mgr = PluginManager::new();
        mgr.register("on-init", "test-init");
        assert_eq!(mgr.hooks.len(), 1);
        assert!(mgr.hooks.contains_key("on-init"));
    }

    #[test]
    fn test_register_multiple_hooks() {
        let mut mgr = PluginManager::new();
        mgr.register("on-init", "test-init");
        mgr.register("on-prompt", "test-prompt");
        mgr.register("on-response", "test-response");
        assert_eq!(mgr.hooks.len(), 3);
    }

    #[test]
    fn test_load_and_eval_janet() {
        let mut mgr = PluginManager::new();
        let result = mgr.eval("(+ 1 2)");
        assert_eq!(result, Ok("3".to_string()));
    }

    #[test]
    fn test_load_and_eval_janet_error() {
        let mut mgr = PluginManager::new();
        let result = mgr.eval("(undefined-fn 1)");
        assert!(result.is_err());
    }

    #[test]
    fn test_dispatch_hook() {
        let mut mgr = PluginManager::new();
        mgr.eval("(defn on-init [ctx] (string \"loaded with model: \" (ctx :model)))")
            .unwrap();
        mgr.register("on-init", "on-init");
        let result = mgr.dispatch("on-init", "@{:model \"gpt-4\"}");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("loaded with model: gpt-4"));
    }

    #[test]
    fn test_harness_log() {
        let mut mgr = PluginManager::new();
        let result = mgr.eval("(harness/log \"hello from plugin\")");
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_file() {
        let mut mgr = PluginManager::new();
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join("test_plugin.janet");
        mgr.load_file(&fixtures).unwrap();
        mgr.register("on-init", "on-init");
        let result = mgr.dispatch("on-init", "@{:model \"test\"}");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("loaded with test"));
    }

    #[test]
    fn test_auto_discover_hooks() {
        let mut mgr = PluginManager::new();
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
        let result = mgr.dispatch("on-init", "@{:model \"test\"}");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("loaded with test"));

        // on-prompt with matching text
        let result = mgr.dispatch("on-prompt", "@{:prompt \"hello world\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "greeting detected");

        // on-prompt with non-matching text
        let result = mgr.dispatch("on-prompt", "@{:prompt \"goodbye\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");

        // on-response with matching text
        let result = mgr.dispatch("on-response", "@{:response \"error: panic\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "error in response");

        // unknown hook returns empty
        let result = mgr.dispatch("on-tool-start", "@{:tool \"bash\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_janet_escaping() {
        let mut mgr = PluginManager::new();

        // Define a test function
        mgr.eval(r#"(defn test-echo [ctx] (ctx :msg))"#).unwrap();
        mgr.register("on-prompt", "test-echo");

        // Quotes in text
        let result = mgr.dispatch("on-prompt", "@{:msg \"he said \\\"hello\\\"\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "he said \"hello\"");

        // Backslashes in text
        let result = mgr.dispatch("on-prompt", "@{:msg \"path\\\\to\\\\file\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "path\\to\\file");

        // Newlines in text
        let result = mgr.dispatch("on-prompt", "@{:msg \"line1\\nline2\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "line1\nline2");
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
        let mut mgr = PluginManager::new();
        mgr.eval(r#"(defn broken [ctx] (string/find "x" nil))"#)
            .unwrap();
        mgr.register("on-prompt", "broken");
        let result = mgr.dispatch("on-prompt", "@{:prompt \"hi\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_dispatch_with_json_args_as_string() {
        // Tool args arrive as JSON; the harness escapes them into a
        // Janet string so the parser never has to handle {":", ","}.
        let mut mgr = PluginManager::new();
        mgr.eval(r#"(defn capture [ctx] (ctx :args))"#).unwrap();
        mgr.register("on-tool-start", "capture");
        let args_json = r#"{"path": "/tmp/x", "n": null, "xs": [1, 2, 3]}"#;
        let ctx = format!(
            "@{{:tool \"Bash\" :args \"{}\"}}",
            escape_janet_string(args_json)
        );
        let result = mgr.dispatch("on-tool-start", &ctx).unwrap();
        assert_eq!(result, args_json);
    }

    #[test]
    fn test_has_symbol() {
        let mut mgr = PluginManager::new();
        mgr.eval("(defn my-hook [ctx] :ok)").unwrap();
        assert!(mgr.has_symbol("my-hook"));
        assert!(!mgr.has_symbol("nope-not-here"));
        // weird names with hyphens/quotes shouldn't crash
        assert!(!mgr.has_symbol("a\"b-c"));
    }

    #[test]
    fn test_janet_phase_tracking() {
        let mut mgr = PluginManager::new();

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
        let result = mgr.dispatch("on-prompt", "@{:prompt \"any\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "entered active");

        // Second prompt: active -> done
        let result = mgr.dispatch("on-prompt", "@{:prompt \"any\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "entered done");

        // Third prompt: done -> nil
        let result = mgr.dispatch("on-prompt", "@{:prompt \"any\"}");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }
}

use std::collections::HashMap;

#[cfg(feature = "plugin")]
use janetrs::client::{Error as JanetError, JanetClient};

/// Escape a Rust string so it can be safely embedded inside a Janet
/// double-quoted string literal. Janet's parser accepts the standard
/// `\"`, `\\`, `\n`, `\r`, `\t` escapes, so we normalise all of those
/// plus any remaining ASCII control characters via `\xNN`.
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

pub struct PluginManager {
    hooks: HashMap<String, Vec<String>>,
    #[cfg(feature = "plugin")]
    client: JanetClient,
    pub phase: String,
    pub pending_prompt: Option<String>,
    pub last_response: Option<String>,
}

impl PluginManager {
    pub fn new() -> Self {
        #[cfg(feature = "plugin")]
        let client = {
            let c = JanetClient::init_with_default_env().expect("Failed to initialize Janet VM");

            // Define harness API functions in Janet
            let _ = c.run(
                r#"
                (var harness-phase :idle)
                (var harness-pending nil)
                (var harness-response nil)

                (defn harness/log [msg] (print "[plugin] " msg))
                (defn harness/get-cwd [] (os/cwd))
                (defn harness/set-phase [p] (set harness-phase p))
                (defn harness/request-prompt [prompt]
                  (set harness-pending prompt))
                (defn harness/store-response [resp]
                  (set harness-response resp))
                (defn harness/has-symbol? [name]
                  (truthy? (get (curenv) (symbol name))))
            "#,
            );

            c
        };

        PluginManager {
            hooks: HashMap::new(),
            #[cfg(feature = "plugin")]
            client,
            phase: String::from("idle"),
            pending_prompt: None,
            last_response: None,
        }
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
    pub fn sync_phase(&mut self) {
        if let Ok(val) = self.client.run("harness-phase") {
            self.phase = val.to_string();
        }
    }

    #[cfg(not(feature = "plugin"))]
    pub fn sync_phase(&mut self) {}

    #[cfg(feature = "plugin")]
    pub fn take_pending_prompt(&mut self) -> Option<String> {
        if let Ok(val) = self.client.run("harness-pending") {
            let s = val.to_string();
            if !s.is_empty() && s != "nil" {
                let _ = self.client.run("(set harness-pending nil)");
                return Some(s);
            }
        }
        None
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
    pub fn dispatch(&mut self, hook: &str, context_janet: &str) -> Result<String, String> {
        let names = match self.hooks.get(hook) {
            Some(names) => names.clone(),
            None => return Ok(String::new()),
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

        Ok(results.join("\n"))
    }

    #[cfg(not(feature = "plugin"))]
    pub fn dispatch(&mut self, _hook: &str, _context_janet: &str) -> Result<String, String> {
        Ok(String::new())
    }
}
