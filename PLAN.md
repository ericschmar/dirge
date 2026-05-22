# Port pi's agent loop to dirge — faithful, phased, TDD

## Scope statement (correcting the earlier plan)

Earlier draft scoped only `prepareNextTurn`. The actual scope is **the
entire pi `runLoop` and surrounding machinery**, ported as closely as
the language difference allows. Pi is the reference; dirge is the
target. Don't invent; port.

Pi's source of truth:
- `~/src/pi/packages/agent/src/agent-loop.ts` (742 LOC — the loop)
- `~/src/pi/packages/agent/src/types.ts` (418 LOC — the type surface)
- `~/src/pi/packages/agent/test/agent-loop.test.ts` (1351 LOC — the spec)

The 21 test cases in pi's `agent-loop.test.ts` are THE SPEC. Every
phase maps to one or more of those tests, ported to Rust and adapted
to dirge's tool/agent abstractions.

## What we're replacing

Currently dirge uses rig's `MultiTurnStream` for the inner loop. Rig
owns turn iteration, tool dispatch, and history management. Dirge
only observes events emitted by rig. This means we CANNOT:

- Swap model/thinking between turns (rig commits both for the stream)
- Inject steering messages between turns
- Apply `prepareNextTurn` / `shouldStopAfterTurn` semantics
- Override tool results via `afterToolCall` cleanly
- Honor `terminate` hints on individual tool results
- Run tools in parallel
- Distinguish sequential-only tools (e.g. bash) from parallel-safe ones
- Provide dynamic API key resolution per request

After the port, dirge owns the loop. Rig provides only the single-turn
LLM call.

## Pi's surface area (what we port)

### Types

| Pi type | Port target |
|---|---|
| `AgentEvent` (12 variants) | `event::AgentEvent` (extend existing) |
| `AgentContext` | `agent::loop::Context` |
| `AgentLoopConfig` | `agent::loop::LoopConfig` |
| `AgentLoopTurnUpdate` | `agent::loop::TurnUpdate` |
| `ShouldStopAfterTurnContext` / `PrepareNextTurnContext` | `agent::loop::TurnHookContext` |
| `BeforeToolCallContext` / `BeforeToolCallResult` | `agent::loop::BeforeToolHook` shapes |
| `AfterToolCallContext` / `AfterToolCallResult` | `agent::loop::AfterToolHook` shapes |
| `AgentTool` (with `executionMode`, `prepareArguments`) | extend `rig::Tool` wrapper |
| `AgentToolResult<T>` (with `terminate`) | new `loop::ToolResult` |
| `ThinkingLevel` | new `event::ThinkingLevel` |
| `ToolExecutionMode` | new `loop::ToolExecutionMode` |
| `QueueMode` | new `loop::QueueMode` |

### Hooks (config callbacks)

| Pi hook | Port (plugin slot OR rust trait) |
|---|---|
| `convertToLlm` | rust closure — converts dirge messages → rig messages |
| `transformContext?` | rust closure — pruning / compression |
| `getApiKey?` | rust closure — dynamic key resolution |
| `shouldStopAfterTurn?` | plugin: `harness/stop-after-turn` + rust hook |
| `prepareNextTurn?` | plugin: `prepare-next-turn` (existing alias) + rust hook |
| `getSteeringMessages?` | UI's interjection_queue + plugin slot |
| `getFollowUpMessages?` | follow-up queue (new) + plugin slot |
| `beforeToolCall?` | existing `on-tool-start` + rust hook |
| `afterToolCall?` | existing `on-tool-end` + rust hook (extend to support full override) |

### Algorithm phases (the loop)

This is the LITERAL algorithm from `runLoop`:

```
runLoop:
  pendingMessages = getSteeringMessages() OR []
  firstTurn = true

  OUTER:
    hasMoreToolCalls = true
    INNER while hasMoreToolCalls OR pendingMessages.nonEmpty():
      if !firstTurn: emit turn_start; else firstTurn = false

      // Inject queued user messages
      drain pendingMessages → emit message_start/end; append to context

      // LLM call
      message = streamAssistantResponse(context, config, signal)
      append message to newMessages

      if message.stopReason in ["error", "aborted"]:
        emit turn_end (toolResults=[]); emit agent_end; return

      // Dispatch tools
      toolCalls = filter(message.content, type=toolCall)
      toolResults = []; hasMoreToolCalls = false
      if toolCalls.nonEmpty():
        batch = executeToolCalls(context, message, config, signal)
        toolResults = batch.messages
        hasMoreToolCalls = !batch.terminate
        for r in toolResults: append to context, newMessages

      emit turn_end (message, toolResults)

      // prepareNextTurn — model/thinking/context swap
      snapshot = config.prepareNextTurn?(ctx)
      if snapshot:
        context = snapshot.context ?? context
        config.model = snapshot.model ?? config.model
        config.reasoning = (snapshot.thinkingLevel undefined ? config.reasoning
                            : snapshot.thinkingLevel == "off" ? None
                            : snapshot.thinkingLevel)

      // shouldStopAfterTurn — graceful stop
      if config.shouldStopAfterTurn?(ctx) == true:
        emit agent_end; return

      // Refresh steering for next iter
      pendingMessages = getSteeringMessages() OR []

    // OUTER: poll follow-up queue
    followUp = getFollowUpMessages() OR []
    if followUp.nonEmpty(): pendingMessages = followUp; continue OUTER
    break OUTER

  emit agent_end
```

### Tool execution (executeToolCalls)

```
prepareToolCall(toolCall):
  tool = lookup by name → if missing: immediate error
  args = tool.prepareArguments?(toolCall.args) ?? toolCall.args   // compat shim
  args = validateAgainstSchema(tool, args)                        // throws → error
  before = config.beforeToolCall?(ctx)
  if signal.aborted: immediate "Operation aborted"
  if before?.block: immediate (reason or default blocked msg)
  return prepared

executePreparedToolCall(prepared):
  result = await tool.execute(id, args, signal, onUpdate)
  // onUpdate emits tool_execution_update events
  catch → error result

finalizeExecutedToolCall(prepared, executed):
  after = config.afterToolCall?(ctx)
  if after: result = { content: after.content ?? result.content,
                       details: after.details ?? result.details,
                       terminate: after.terminate ?? result.terminate }
            isError = after.isError ?? isError
  catch → error result

executeToolCallsSequential / executeToolCallsParallel:
  // Sequential = await per call; Parallel = Promise.all on prepared lambdas
  // Per-tool executionMode=sequential forces sequential
  // emit tool_execution_start BEFORE prepare; tool_execution_end AFTER finalize
  // emit message_start/end for tool-result message AFTER (parallel: in source
  //   order even if finalize completed out-of-order)
  return { messages, terminate: every result has terminate==true }
```

---

## Phasing

Each phase ships green tests + green build. Tests are ported directly
from pi's `agent-loop.test.ts`. Each test name in this plan corresponds
to a `it("should …")` in pi's file.

### Phase 0 — Scaffolding (no behavior change)

**Goal**: introduce the new types and the empty new-loop module
behind a feature flag `new-loop`. Nothing uses them yet; default
build is unchanged.

**Files**:
- `src/agent/agent_loop/mod.rs` (new) — empty module, public types
- `src/agent/agent_loop/types.rs` — `Context`, `LoopConfig`, `TurnUpdate`,
  hook context structs, `ThinkingLevel`, `ToolExecutionMode`, `QueueMode`
- `src/agent/agent_loop/tool.rs` — `LoopTool` trait with
  `execute(id, args, signal, on_update)`, `prepare_arguments`, `execution_mode`
- `src/agent/agent_loop/result.rs` — `LoopToolResult { content, details, terminate }`
- `Cargo.toml` — `new-loop = []` feature

**Tests** (pure type-level): roundtrip serde of `ThinkingLevel`,
`ToolExecutionMode`, default values.

**Risk**: zero. Code is unreachable until phase 3.

---

### Phase 1 — `streamAssistantResponse` analog

**Goal**: single-turn LLM call wrapper around `rig::agent::Agent::prompt`
that produces an `AssistantMessage`-equivalent + emits dirge events.
The leaf of pi's loop.

**Files**:
- `src/agent/agent_loop/stream.rs` (new) — `stream_assistant_response`
- Reuse existing event emit logic; emit `Token`, `Reasoning`, etc.
- Resolve API key dynamically via `LoopConfig::get_api_key`
- Apply `transform_context` if configured
- Apply `convert_to_llm` (Required)
- Return a `FinalAssistantMessage { content, stop_reason, error_message }`

**Tests (port from pi)**:
- `should emit events with AgentMessage types` (line 84) — single LLM
  call emits start → updates → end
- `should handle custom message types via convertToLlm` (131) —
  convertToLlm filters/maps non-LLM message types
- `should apply transformContext before convertToLlm` (186) —
  transform sees the raw transcript first

**Risk**: medium. Bridge from rig's single-turn API to pi's
event vocabulary. Needs a mock rig agent for tests.

---

### Phase 2 — Tool execution: sequential

**Goal**: port `executeToolCallsSequential` + `prepareToolCall` +
`executePreparedToolCall` + `finalizeExecutedToolCall`. Wires
beforeToolCall, afterToolCall, terminate, prepareArguments.

**Files**:
- `src/agent/agent_loop/tools.rs` (new) — sequential dispatcher
- `src/agent/agent_loop/hooks.rs` (new) — `BeforeToolHook`,
  `AfterToolHook` traits with closure adapters

**Tests (port from pi)**:
- `should handle tool calls and results` (239)
- `should execute mutated beforeToolCall args without revalidation` (310)
- `should prepare tool arguments for validation` (372) — `prepareArguments`
  shim runs BEFORE schema validation; `beforeToolCall` mutates AFTER
- `should stop after a tool batch when every tool result sets
  terminate=true` (1067)
- `should allow afterToolCall to mark a tool batch as terminating` (1184)

**Risk**: medium. Hook contract has subtle ordering (prepareArguments →
validate → beforeToolCall → execute → afterToolCall).

---

### Phase 3 — Tool execution: parallel + per-tool sequential override

**Goal**: port `executeToolCallsParallel`. Tools that declare
`executionMode == "sequential"` (e.g. `bash`) force the whole batch
sequential even with default parallel config.

**Files**:
- `src/agent/agent_loop/tools.rs` — add parallel dispatcher
- `src/agent/tools/bash.rs` — set `executionMode: Sequential`
- `src/agent/tools/edit.rs`, `write.rs`, `apply_patch.rs` — sequential
  (they touch the filesystem and could race)

**Tests (port from pi)**:
- `should emit tool_execution_end in completion order but persist
  tool results in source order` (452) — KEY parallel-correctness test
- `should force sequential execution when a tool has
  executionMode=sequential even with default parallel config` (653)
- `should force sequential execution when one of multiple tools has
  executionMode=sequential` (736)
- `should allow parallel execution when all tools have
  executionMode=parallel` (823)
- `should continue after parallel tool calls when not all tool results
  terminate` (1119)

**Risk**: high. Concurrent borrow management for tools that hold &mut
references. May need `Arc<Mutex<…>>` or per-tool state cloning.
Permission checker calls + ask channel need to be parallel-safe (probably
already are; verify).

---

### Phase 4 — The loop itself (`runLoop`)

**Goal**: port `runLoop` and `runAgentLoop` / `runAgentLoopContinue`.
This is the keystone. After this phase the new-loop feature ships
behavior-equivalent runs through the new path.

**Files**:
- `src/agent/agent_loop/run.rs` (new) — `run_loop`, `run_agent_loop`,
  `run_agent_loop_continue`
- `src/agent/agent_loop/queue.rs` (new) — steering queue + follow-up
  queue with `QueueMode` (drain-all vs one-at-a-time)
- `src/agent/runner.rs` — feature-gated dispatch: under `new-loop`,
  delegate to `agent_loop::run_loop`; otherwise keep existing rig
  multi-turn path

**Tests (port from pi)** — the meat of the spec:
- `should use prepareNextTurn snapshot before continuing` (897) —
  model/thinking/context all swap correctly
- `should stop after the current turn when shouldStopAfterTurn
  returns true` (970)
- `should inject queued messages after all tool calls complete` (547) —
  steering ordering invariant
- `agentLoopContinue` cases (1233-1351)

**Risk**: HIGH. Keystone. The retry/recovery loop currently wraps
the whole stream — needs to wrap each single-turn call instead.
Interjection currently fires at rig's tool-result boundary —
needs to map onto pi's steering queue mechanism.

Mitigation: keep the existing path behind `--features !new-loop`
default until phase 4 passes the full ported spec. Flip default in
phase 4.5 (separate commit) after baking.

---

### Phase 4.5 — Integration: rig + dirge consumers → ported loop

**SCOPE CORRECTION**: The original 4.5 was wildly underscoped as
"delete the old path." Flipping the default is actually a multi-
commit integration project — the new loop currently works only
with mock streams + mock tools. Sub-divided below.

Each sub-phase ships green builds + green tests + a concrete
integration validation. Order is roughly dependency-ordered;
later ones can interleave once their inputs land.

#### 4.5a — rig → StreamFn adapter

**Goal**: build a `StreamFn` that wraps rig's single-turn API
(`agent.prompt(history)` or equivalent) so the loop can drive a
real LLM. This is the "physical layer" — without it the loop is
a beautiful algorithm with nothing to talk to.

**Files**:
- `src/agent/agent_loop/rig_stream.rs` (new) — `rig_stream_fn`
  factory taking a rig agent and producing a `StreamFn`

**Tests**: integration test that calls a stubbed rig provider
through the adapter. Doesn't need a real network — rig has its
own mock fixtures.

**Risk**: medium. Rig's stream-event vocabulary needs mapping
to our `StreamEvent` enum. Tool-call extraction is subtle.

#### 4.5b — rig::Tool → LoopTool adapter

**Goal**: a generic wrapper that takes any `Box<dyn rig::Tool>` and
implements `LoopTool` on it. Every dirge tool (read / write / edit /
bash / grep / find_files / list_dir / apply_patch / semantic_* /
mcp_* / skill) gets a LoopTool surface for free.

**Files**:
- `src/agent/agent_loop/rig_tool.rs` (new) — `RigToolAdapter` struct
  + impl
- `src/agent/builder.rs` — alongside the existing `Vec<Box<dyn
  rig::ToolDyn>>` build, also produce `Vec<Arc<dyn LoopTool>>`
  via the adapter

**Tests**: wrap a real dirge tool (e.g. `read`), execute through
the LoopTool surface, verify identical output to the rig path.

**Risk**: medium. rig::Tool's args/output types are generic;
LoopTool uses `Value`. The adapter does the serde round-trip.

#### 4.5c — LoopEvent → AgentEvent translation

**Goal**: bridge the loop's event vocabulary to dirge's existing
`AgentEvent`. The UI and ACP consume `AgentEvent`; until we
replace those, we translate at the boundary.

**Files**:
- `src/agent/agent_loop/bridge.rs` (new) — `translate_event(LoopEvent)
  -> Vec<AgentEvent>` (Vec because some pi events split into multiple
  dirge events — `MessageUpdate` with `phase=TextDelta` → `Token`)

**Tests**: round-trip every LoopEvent variant; assert the
AgentEvent stream matches what the old runner emits for an
equivalent algorithmic input.

**Risk**: low. Pure data translation. Easy to verify.

#### 4.5d — Plugin slots → before/after tool hooks

**Goal**: dirge's existing plugin hooks (`harness/block`,
`harness/mutate-input`, `harness/replace-result`) become
`BeforeToolCallFn` / `AfterToolCallFn` closures consumed by the loop.

**Files**:
- `src/agent/agent_loop/plugin_hooks.rs` (new) — factory functions
  `before_hook_from_plugin_manager(&PluginManager) -> BeforeToolCallFn`
  etc.

**Tests**: install a Janet plugin that mutates args; verify the
tool sees mutated args end-to-end through the loop.

**Risk**: medium. Plugin hooks fire from async closures; the
PluginManager's locking pattern needs verification.

#### 4.5e — interjection_queue → getSteeringMessages

**Goal**: dirge's existing `interjection_queue` (in `ui/mod.rs`)
becomes the source for `GetSteeringMessagesFn`.

**Files**:
- `src/agent/agent_loop/steering.rs` (new) — factory that takes a
  shared `Arc<Mutex<VecDeque<String>>>` and produces a
  `GetSteeringMessagesFn`

**Tests**: enqueue messages, run a loop, verify they're injected at
the next turn boundary.

**Risk**: low.

#### 4.5f — runner.rs replacement (BEHIND a flag)

**Goal**: add a `--use-agent-loop` CLI flag (or config option) that
routes a run through the new loop instead of the rig multi-turn
stream. Both paths coexist; default is still the old path.

**Files**:
- `src/agent/runner.rs` — branch on the flag; dispatch to either
  `spawn_runner` (existing) or a new `spawn_runner_via_loop` that
  composes 4.5a + 4.5b + 4.5c + 4.5d + 4.5e

**Tests**: integration test that runs a multi-turn session through
the new path; assert identical observable behavior (event stream
matches) for a canned scenario.

**Risk**: high. First time the new loop touches real dirge state.
Expected edge cases: agent_line_started state, chamber rendering,
permission check signal threading.

#### 4.5g — Recovery / retry under the new path

**Goal**: wrap each `stream_assistant_response` call with the existing
`recovery::classify_error` + Retry-After backoff. Verify auto-compact
on `ContextOverflow` works.

**Files**:
- `src/agent/agent_loop/retry.rs` (new) — `retrying_stream_fn` wrapper
  that intercepts errors and retries with the recovery policy

**Tests**: simulate a network error mid-turn; verify retry happens.

**Risk**: medium.

#### 4.5h — Flip default; delete old path

**Goal**: make the new loop the only path. Delete the old
rig-multi-turn consumer code. This is the ORIGINAL 4.5 description
— but it now lands AFTER everything above proves the new path works.

**Files**:
- `src/agent/runner.rs` — strip the rig multi-turn path
- `Cargo.toml` — remove the `agent-loop` feature flag
- `src/agent/agent_loop/mod.rs` — remove the `#![allow(dead_code)]`
  + `#![allow(unused_imports)]` module-level allows

**Tests**: all 724+ default tests still pass.

**Risk**: medium. Deletion. Anything subtle that survived 4.5a-g
should now be the ONLY behavior.

---

### Phase 5 — Plugin hook wiring

**Goal**: surface every pi hook to Janet plugins via the existing
slot mechanism. Auto-applied at the right loop points.

**Slots (port from pi semantics)**:
- `harness-next-model` → `prepareNextTurn.model` (already exists; just
  re-wired)
- `harness-next-thinking-level` → `prepareNextTurn.thinkingLevel`
- `harness-next-context-system-prompt` / `harness-next-context-messages`
  → `prepareNextTurn.context`
- `harness-stop-after-turn` → `shouldStopAfterTurn` (drained per turn)
- `harness-steering-messages` → `getSteeringMessages` (drained per
  turn)
- `harness-followup-messages` → `getFollowUpMessages` (drained at
  outer-loop boundary)

**Janet helpers**:
- `harness/set-next-thinking-level` `(low|medium|high|xhigh|off|minimal)`
- `harness/request-stop-after-turn`
- `harness/add-steering` `(content)`
- `harness/add-followup` `(content)`

**Tests**: each slot has a Janet integration test (set slot from
on-tool-end hook → verify behavior on next turn).

**Risk**: low. Slot mechanism is well-trodden in dirge.

---

### Phase 6 — Recovery + interjection + abort under new loop

**Goal**: make sure every existing dirge feature works under the new
loop architecture. This is the long-tail hardening phase.

**Specific paths**:
- `recovery::classify_error` wrapping each `stream_assistant_response`
  call (not the whole run)
- `Retry-After` header parsing still works
- Network error → backoff → resume preserves history
- Auto-compact on `ContextOverflow` → retry through the new loop
- Ctrl+C interrupts the in-flight LLM stream cleanly
- Ctrl+C while a tool runs aborts via the AbortSignal-equivalent
- `Esc Esc` rewind
- `/quit` mid-run
- Tool permission deny → tool result with denial message → next turn
  proceeds normally

**Tests**: regression suite that replays canned event sequences and
asserts identical observable behavior to the pre-port baseline.

**Risk**: medium. Edge cases.

---

### Phase 7 — Custom message types (`CustomAgentMessages`)

**Goal**: pi allows app-defined non-LLM message variants
(notifications, artifacts) that `convertToLlm` filters before sending
to the model. Port the abstraction so dirge plugins can inject UI-only
messages without polluting the LLM context.

**Files**:
- `src/agent/agent_loop/messages.rs` — `LoopMessage` enum extension
  point; default `convert_to_llm` filters non-LLM variants
- Plugin slot: `harness-custom-message` to push UI-only messages
- UI consumer renders custom messages in chat without sending to model

**Tests**:
- `should handle custom message types via convertToLlm` (131)
- Verify custom messages reach the UI but not the LLM

**Risk**: low. Additive; no existing behavior changes.

---

### Phase 8 — Polish, parity verification, deprecations

**Goal**: every test in pi's `agent-loop.test.ts` passes its dirge
counterpart. Deprecation cleanup.

**Tasks**:
- Audit pi's test file; verify each `it(…)` has a corresponding
  passing test in dirge
- Deprecate `prepare-next-run` (alias for `prepare-next-turn`); emit
  warning when used; remove in next minor
- Update README / docs to describe the new hook surface
- Add a `docs/agent-loop.md` walkthrough mirroring pi's algorithm

**Tests**: parity assertion test that diff'ing dirge's loop algorithm
against pi's `runLoop` finds no semantic gaps.

**Risk**: low. Documentation + parity verification.

---

## Verification gates per phase

Before each phase commits:
1. `cargo build --all-features` clean
2. `cargo test` green
3. `cargo fmt --check` clean
4. The phase's ported pi tests pass
5. Existing tests still pass
6. PLAN.md updated to mark the phase ✅

## Commit cadence

One phase per commit, except phases 4 and 4.5 which are paired (4
introduces, 4.5 flips default). Each commit:
- Title: `feat(agent): phase N — <one-line goal>`
- Body cites the pi test cases ported and any honest scope notes
- No commit ships untested behavior

## Estimated LOC

| Phase | LOC | Tests added |
|---|---|---|
| 0 | ~250 | 5 |
| 1 | ~350 | 3 |
| 2 | ~400 | 5 |
| 3 | ~350 | 5 |
| 4 | ~500 | 4 |
| 4.5 | -300 (deletion) | 0 (existing) |
| 5 | ~250 | 6 |
| 6 | ~150 | 8 |
| 7 | ~200 | 2 |
| 8 | ~100 | 1 |
| **Total** | **~2250** | **39** |

## Out-of-scope (not in this plan, may be follow-up plans)

- **Pi's `AgentHarness`** (`packages/agent/src/harness/agent-harness.ts`,
  995 LOC) — the higher-level "agent harness" wrapping `agentLoop`
  with compaction, retry, session management. Dirge already has its
  own equivalents (recovery, compact, session). Could be re-evaluated
  after the core loop port lands.
- **Pi's compaction policy** (`harness/compaction/compaction.ts`) —
  dirge has its own `/compress` + auto-compact. Could be ported
  separately if a specific divergence emerges.
- **Pi's skills system** — dirge has its own skill discovery; not
  the same shape.
- **Pi's `StreamFn` injection** — dirge uses the rig provider
  abstraction. The port uses rig's single-turn API throughout.

These four items are dirge's existing equivalents and don't need
porting for the loop change. They may diverge in subtle behavior
from pi, but those divergences are isolated from the `runLoop` port.

## Risk summary

| Phase | Risk | Why |
|---|---|---|
| 0 | None | Pure scaffolding |
| 1 | Med | Bridges rig single-turn API → pi event vocab |
| 2 | Med | Hook ordering subtlety |
| 3 | **High** | Concurrent tool dispatch + borrow management |
| 4 | **High** | The keystone; retry/recovery wrap point changes |
| 4.5 | Med | Deleting the old path; nothing left to fall back to |
| 5 | Low | Slot mechanism well-trodden |
| 6 | Med | Edge cases under integration |
| 7 | Low | Additive |
| 8 | Low | Documentation |

## Order of operations

Strict linear: 0 → 1 → 2 → 3 → 4 → 4.5 → 5 → 6 → 7 → 8.

Phases 1 / 2 / 3 are independent in spirit but share the type
surface from phase 0. Phases 1 / 2 / 3 must all land before phase 4
because the loop calls into all three.

Phase 6 can interleave with 5 if a particular hook integration
surfaces an unrelated edge case, but the default order is 5 → 6.
