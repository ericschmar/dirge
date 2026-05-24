# DeepSeek Tool-Input Repair

Why dirge needs a repair layer for open-model tool calls, what the
common failure modes look like, and how each one maps to dirge's
agent-loop dispatch path. Distilled from a multi-day production study
on DeepSeek-flash and DeepSeek-v4-pro (billions of tokens through a
sibling CLI), with cross-checks against GLM and Qwen.

The headline finding: with a small repair layer in place, DeepSeek
v4-pro beat Opus 4.7 6/10 on the sibling tool's internal evals.
**The model didn't change. The harness got more forgiving in the
places it needed to.**

## TL;DR

| Category | Symptom | Where to fix in dirge |
|---|---|---|
| Shape mismatch | `null` for optional, JSON-string instead of array, single-arg `{}` instead of array, bare string instead of array | New repair pass between `RigToolAdapter::execute` and `rig::ToolDyn::call` (`src/agent/agent_loop/rig_tool.rs:147-184`) |
| Path leakage | `[notes.md](http://notes.md)` markdown auto-link in path fields | Schema-driven path field normalizer (degenerate-link case only) |
| Relational invariant | `read_file({path, limit:30})` errors because schema demands offset/limit pair | Per-tool execute: default missing half, surface decision in result without `Error:` prefix |
| Error opacity | Raw `serde_json::Error` text shown verbatim to model — unrepairable from the model's side | `format_tool_error` (`src/agent/agent_loop/rig_tool.rs:192`) should translate JSON errors into actionable hints |

## The four shape failures

These are not random. Across DeepSeek-flash, DeepSeek-v4-pro, GLM,
and Qwen, the same four shape mistakes account for ~90% of
deserialization failures:

1. **`null` for optional field.** The model emits
   `{"path": "x", "offset": null}` instead of omitting `offset`.
   Serde with `Option<T>` typically accepts both, but some derivations
   error on null where a missing key would succeed. Repair: strip
   `null`-valued keys whose schema says optional.

2. **JSON-string instead of array.** The model emits
   `{"paths": "[\"a\",\"b\"]"}` — a `String` containing the JSON of
   the intended array. Repair: when schema at this path says `array`
   but input is `string` matching `^\s*\[.*\]\s*$`, attempt
   `serde_json::from_str::<Value>(s)` and substitute if the parse is
   an array.

3. **Empty-placeholder object instead of array.** The model emits
   `{"items": {}}` when the schema wants `items: array`. Repair:
   when schema says `array` and input is the empty object `{}`,
   substitute `[]`.

4. **Bare string instead of array-of-string.** The model emits
   `{"paths": "foo"}` when schema wants `paths: ["foo"]`. Repair:
   when schema says `array` of `string` (or any array) and input is
   a string, wrap as singleton array `[input]`.

**Order matters.** JSON-string parse (#2) must run BEFORE
bare-string wrap (#4), or `"[\"a\",\"b\"]"` becomes
`["[\"a\",\"b\"]"]` — a singleton array containing the original
JSON string. The sibling tool burned a debugging session on this.

## The funniest failure: markdown auto-link in path fields

DeepSeek-flash, when asked to write a file, sometimes emits:

```
{"path": "/Users/x/proj/[notes.md](http://notes.md)", "content": "…"}
```

Without a fix, dirge's write tool faithfully creates a file named
`[notes.md](http://notes.md)`. This is post-training leak: the model
was rewarded for auto-linking in chat output, and applies the prior
when it crosses the tool boundary.

**Repair (degenerate case only):** if a path-typed field matches
the regex `^\[(.+?)\]\(https?://\1\)$` or
`^\[(.+?)\]\(https?://([^/]*\.)?(.+?)\)$` where the link text
equals (or is a sub-path of) the URL stripped of protocol, unwrap
to the link text. Real markdown like `[click](https://example.com)`
where the text and URL are semantically different MUST pass through
untouched — it's not a path, but if it ever appears in a path
field, leaving it alone surfaces the bug rather than silently
mutating real data.

**Schema-driven, not blanket.** Apply only to fields the JSON
Schema marks as path-like — by name (`path`, `file_path`,
`filename`, etc.) or by a custom `x-dirge-kind: path` extension.
Never to free-form `content` / `text` / `body` fields.

## Validate-then-repair, not preprocess-then-validate

The naive design is a preprocessing pass: walk the args, strip
nulls, parse stringified arrays, etc. Then validate.

**This is wrong.** It encodes a prior about what's broken that
applies even when nothing is. The sibling tool tried it; valid
inputs whose `content` field happened to be JSON-shaped got
rewritten on disk. Silent corruption, easy to miss in a smoke test.

**The right design** inverts the order:

1. **Try the input as-is.** Hand the raw `Value` to
   `rig::ToolDyn::call`. If deserialization succeeds, the call
   ships unchanged. **Valid inputs are never touched.**

2. **On failure, localize.** Walk the validator's issue list —
   each issue has a path (`/items/0/path`) and a complaint (`expected
   array, found string`). For each issue path, try the four
   repairs in order until one applies at that specific path.

3. **Retry once.** Parse again. On success, log
   `tool_input_repaired: {tool, model, repair_kind}`. On failure,
   log `tool_input_invalid: {tool, model}` and return a
   model-readable retry message.

The validator is doing the work of localizing the bug for you.
You spend repair budget only at the exact paths the schema
disagreed at.

**Gap in dirge today:** rig owns deserialization (line 162) and
returns `ToolError` without the issue list. To get validate-then-
repair semantics, dirge needs to either:
- pre-validate the JSON against the tool's `parameters()` schema
  itself (jsonschema crate or hand-rolled walker) before calling
  rig, OR
- catch the error path, parse the error text to extract the
  failing JSON pointer, and apply repairs there.

The first is cleaner.

## Relational invariants

Shape repairs don't help when each field is independently valid
but the *combination* is wrong. The sibling tool hit this on
`read_file`:

```
read_file({path: "...", limit: 30})
→ Error: limit requires offset (and vice versa)
```

Each field is fine. The error is in the relationship.

**Fix at the tool implementation level, not the harness:**

- `limit` alone → `offset = 0`. Read from the start.
- `offset` alone → `limit = 2000`. The sibling tool uses 2000 as
  its "default chunk"; dirge can pick its own value (the existing
  `ReadArgs` has `offset: Option<usize>`, `limit: Option<usize>` —
  both defaults should land at sensible values rather than
  erroring).
- Surface the decision in the tool *result* (not as `Error:`):

  > Note: limit was not provided; defaulted to 2000 lines. To
  > read more or fewer, retry with both `offset` and `limit`.

  No `Error:` prefix means the TUI doesn't paint it red. The model
  sees what was chosen and can self-correct on the next turn.

**Audit other tools for similar pairs.** Candidates to review:
- `grep` — `pattern` + `context_lines` (probably fine — they're
  independent)
- `bash` — `command` + `timeout` (independent)
- `edit` — `old_text` + `new_text` + `replace_all` (independent;
  `replace_all` is a flag)

Most dirge tools are single-arg or independent-field. `read_file`
is the standout candidate for relational defaulting.

## Error formatting

When repair fails, the model needs an actionable message — not
the raw `serde_json::Error`. Today
(`src/agent/agent_loop/rig_tool.rs:192`):

```rust
fn format_tool_error(err: ToolError) -> String {
    err.to_string()
}
```

This leaks `missing field 'path' at line 1 column 10` to the
model. That's a parser's error message; the model can't repair
from it because it doesn't carry the JSON Schema context.

**Proposed format:**

```
Tool input rejected: <plain English summary>
Expected: <relevant slice of the JSON Schema>
Got:      <the value at the failing path, truncated>
Try:      <one concrete hint, e.g. "remove the offset field, or
           pair it with limit">
```

The model recovers from this far more reliably than from
`expected ',' or '}' at line 3 column 25`.

## Telemetry

Once the repair layer is in place, log per-call:

- `(model, tool, repair_kind | "none" | "failed")`

This gives free regression detection. If DeepSeek-v4-pro suddenly
starts triggering `BareStringToArray` for a tool it never used to,
something changed in the model's post-training distribution and
you'll see it before users do.

Suggested log target: `tracing::info!(target: "tool_repair", …)`
with the existing tracing subscriber.

## Implementation plan

Concrete sequencing for the dirge harness. Each phase is
independently mergeable and lands in a separate PR.

### Phase 1 — repair layer (the four shape fixes)

- **Where:** new `src/agent/agent_loop/tool_input_repair.rs`.
- **Hook point:** `RigToolAdapter::execute` at
  `src/agent/agent_loop/rig_tool.rs:147-184`. Wrap the
  `inner.call(args_string)` call: on `Err`, run the repair
  walker against the original `Value` (we still have it), retry
  once, log the outcome.
- **Schema source:** `self.parameters` already holds the JSON
  Schema as a `serde_json::Value`. The walker reads field
  types from it.
- **Repairs in order:** null-strip → JSON-string-to-array →
  empty-object-to-array → bare-string-to-array.
- **Tests:** unit tests with synthetic schemas + the four
  failure shapes. Pin the ordering bug
  (`'["a","b"]'` must become `["a","b"]`, not `['["a","b"]']`).

### Phase 2 — markdown auto-link unwrap

- **Where:** same module, behind a separate `unwrap_md_link`
  helper.
- **Field selection:** by JSON pointer prefix using a small
  static list of known path-like field names (`path`,
  `file_path`, `filename`, `paths`, `dir`) AND any field
  carrying a custom `x-dirge-kind: path` schema annotation.
- **Tests:** real markdown links pass through; degenerate
  auto-links are unwrapped; nothing happens for non-path
  fields even if they contain a markdown link.

### Phase 3 — read_file relational defaulting

- **Where:** the read tool's `Tool::call` implementation
  (find via `pub struct ReadArgs` at
  `src/agent/tools/mod.rs:130`).
- **Behaviour:** when only one of `offset` / `limit` is
  present, fill the other. Prepend a `Note:` line to the
  result body so the model sees the chosen default.
- **Tests:** all four combinations (neither, offset-only,
  limit-only, both) produce a successful read with the
  expected default surfaced.

### Phase 4 — actionable error formatting

- **Where:** `format_tool_error` at
  `src/agent/agent_loop/rig_tool.rs:192`.
- **Behaviour:** parse the underlying error category. For
  `ToolError::JsonError`, extract the failing path from the
  serde message (regex on `at line N column M` + a JSON
  pointer if possible), and emit the structured retry hint
  shown above.

### Phase 5 — telemetry

- `tracing::info!(target: "tool_repair", model = %model_name,
  tool = %tool_name, repair = ?repair_kind)` at the repair
  exit.
- Existing log filter setup will pick this up under
  `RUST_LOG=tool_repair=info` or `--verbose`.

## Frame shift

The article that prompted this work closes with: "skill issue
applies to the harness more often than the model." Worth keeping
near the top of the head when triaging a model regression:

- A strict schema is a *choice* with a cost. It filters out
  noise, AND it filters out recoverable noise.
- Large commercial models eat that cost invisibly because they've
  seen enough of every contract during pretraining.
- Open models pay it loudly and get dismissed as "bad at tool
  calls."
- The harness is the mediation layer between distributions.
  Small, targeted forgivenesses where the contract is needlessly
  strict close the gap.
