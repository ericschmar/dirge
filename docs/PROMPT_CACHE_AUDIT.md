# Prompt-Cache Positioning Audit

Phase 3 part 3 of `AGENTIC_LOOP_PLAN.md`: verify the cacheable
prefix (system prompt + tool defs) lands at the start of every
LLM request, dynamic content (history + current prompt) at the
end, and that nothing per-turn perturbs the prefix.

## Request shape

The request is assembled in
`src/agent/agent_loop/rig_stream_factory.rs:171-192`:

```rust
let mut builder = CompletionRequestBuilder::new(model, prompt);
if !ctx.system_prompt.is_empty() {
    builder = builder.preamble(ctx.system_prompt);   // ← cacheable
}
builder = builder.messages(history);                  // ← grows per turn
if !tools.is_empty() {
    builder = builder.tools((*tools).clone());        // ← cacheable
}
let request = builder.build();
```

The rig Anthropic adapter (`rig-core-0.37.0`,
`providers/anthropic/completion.rs`) serialises these to the
Anthropic Messages API as:

```json
{
  "system": "<preamble>",
  "tools":  [...],
  "messages": [...]
}
```

Anthropic's auto-cache treats `system` + `tools` as the prefix and
`messages` as the suffix. **Positioning is correct.**

## Stability of the cacheable prefix

### `system_prompt`

Source: `provider/mod.rs:871-876` (the `spawn_runner` path):

```rust
let system_prompt = if history_preamble.is_empty() {
    self.preamble.clone()
} else {
    format!("{}\n\n{}", self.preamble, history_preamble)
};
```

- `self.preamble` is built **once per session** in
  `agent::builder::build_agent_inner` and stored on
  `BoxedRigAgent`. It includes: static SYSTEM_PROMPT,
  TODO_TOOLS_PROMPT, agents file, current prompt frontmatter,
  cwd, OS, shell, git branch, memory store contents, project
  skills list, and mode reminders. **None of these change
  mid-session** — they're snapshotted at agent build.
- `history_preamble` is `rig_history_system_prompt(history)` —
  concatenates any `Message::System` entries in history. The
  only path that injects mid-conversation System messages is
  **compaction** (Hermes-style summarisation). When compaction
  fires, the prefix mutates by design.

**Verdict**: stable except across compaction events. Compaction
intentionally trades a cache miss for context savings — that's
the right tradeoff.

### `tools`

Source: `agent::builder::build_loop_tools`. The Vec is built by a
linear sequence of `tools.push(...)` calls; built-in tool order
is deterministic. MCP and semantic-plugin tools are appended in
the iteration order of the manager — also deterministic per
session (HashMap iteration in Rust is randomised per-process but
stable per-instance once populated; the manager builds its
listing once at session start).

**Verdict**: stable within a session. The same `Arc<Vec<…>>` is
re-used per turn (`cfg.tools = self.loop_tools` —
`provider/mod.rs:885`), so the per-turn `(*tools).clone()` in
the factory copies an unchanged Vec.

### History prefix

Append-only **except** when compaction rewrites it. The append
path doesn't reorder; the model's previous turn's tool-calls and
tool-results stay byte-identical on the next turn.

## Recommendations

1. **Already in good shape.** Auto-caching kicks in on the
   correct prefix on Anthropic. We pay the cache miss only when
   compaction fires, which is desirable.

2. **Add prefix-hash telemetry** (this commit). Hash
   `system_prompt + serde_json::to_string(&tools)` and emit a
   `prompt_cache_prefix` tracing event each turn. External
   analysis (or a future dashboard) can detect unexpected
   prefix drift — e.g. someone accidentally puts a timestamp in
   the preamble.

3. **Defer explicit `cache_control` markers.** Rig exposes
   `CacheControl::ephemeral()` in its Anthropic provider
   (`completion.rs:187`), but threading it through
   `CompletionRequestBuilder` requires either:
   - Provider-specific `additional_params` (fragile — assumes
     rig's internal JSON shape), or
   - Upstream rig changes to expose cache markers on the
     generic builder.

   Anthropic's auto-cache works without explicit markers when
   the prefix is stable, so this is a small additional win
   (explicit markers give finer TTL control). Track as a
   follow-up.

4. **Watch for compaction-thrash.** If `Phase 5` work introduces
   automatic compaction (currently it's manual / context-pressure
   triggered), each compaction event burns the cache. Consider
   batching multiple turns of "summarisable" content before
   firing compaction so the cache miss amortises across more
   future turns.

## Future-proofing assertion

The factory now emits a `prompt_cache_prefix` tracing event each
call with the SHA-256 hashes of:
- `system_prompt`
- the serialised tool list (sorted by name to defeat HashMap
  iteration randomness across processes)
- the count of history messages

A future refactor that accidentally reshuffles
(e.g. moves cwd-injection from `build_agent_inner` to the
per-turn factory) will surface as the system-prompt hash
changing across turns — visible in the tracing log.
