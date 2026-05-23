# Plugin-registered LLM-callable tool example (P9a).
#
# Demonstrates `harness/register-tool`, which lets a plugin add a tool
# the LLM can call alongside the built-ins. Pi-style API mirror —
# `api.registerTool(...)` in pi's extension surface.
#
# Signature:
#   (harness/register-tool name description label parameters handler &opt execution-mode)
#
# - name        — what the LLM sees / refers to in tool calls
# - description — shown to the LLM in the tool list; should explain
#                 when to use the tool and the expected argument shape
# - label       — UI display name (chat banner). Falls back to `name`.
# - parameters  — JSON-schema string. The host parses once at startup;
#                 invalid JSON drops the tool with a tracing::warn.
# - handler     — name of the Janet function that runs the tool. It
#                 receives the LLM's raw JSON-args string as its single
#                 argument and returns the result (string preferred;
#                 other types are coerced via Janet's `(string ...)`).
# - execution-mode — :parallel (read-only, default) or :sequential
#                    (mutating). Forces the whole batch sequential when
#                    set — matches pi's `hasSequentialToolCall` rule.

(defn echo-tool-handler [args]
  # `args` is the raw JSON string the LLM produced. Plugins that want
  # structured fields parse it themselves — Janet's bundled runtime
  # has no JSON decoder, but the string is often enough for display
  # or pass-through behavior.
  (string "echo received args: " args))

(harness/register-tool
  "plugin_echo"
  "Echoes the args back verbatim. Use this when you want to verify tool plumbing."
  "Plugin Echo"
  "{\"type\":\"object\",\"properties\":{\"msg\":{\"type\":\"string\"}},\"required\":[\"msg\"]}"
  "echo-tool-handler")
