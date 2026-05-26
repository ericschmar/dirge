# Implementation Plan: DeepSeek-Optimized Agentic Coding Harness

**Document Version:** 1.0  
**Target Model:** DeepSeek series (Flash, V4 Pro, etc.)  
**Objective:** Build a resilient harness that systematically addresses common DeepSeek tool‑calling failures, turning a “skill issue” into a structural advantage. The harness must be model‑agnostic in architecture but deeply informed by DeepSeek’s specific error distribution.

---

## 1. Overview & Design Philosophy

The core insight from research is:

> *“Tool confusion is a harness problem, not a model problem. The harness mediates between the model’s chat distribution and the system’s strict contracts.”*

Rather than retrain or prompt‑engineer away every mistake, the harness will **observe, localise, and repair** – or transparently extend semantics – with zero latency impact on valid inputs. Every repair is logged, creating a feedback loop for continuous improvement.

We will adopt a **validate‑then‑repair** pattern:  
```
Raw input → Validate → Success? → Execute  
                     → Failure? → Walk validator issue list → Apply targeted repair → Retry validation → Log & Execute or surface error.
```

---

## 2. Feature Catalogue

### 2.1 Tool‑Input Repair Layer (TI‑Repair)

This is the heart of the system, directly implementing the four repairs discovered in the DeepSeek failure catalogue.

**2.1.1 Compositional Repairs** (ordered by dependency)

| Repair Name | Trigger | Action | Example |
|-------------|---------|--------|---------|
| `json-array-parse` | String that looks like a JSON array when schema expects array | `JSON.parse()` the string | `"['a','b']"` → `["a","b"]` |
| `null-field-strip` | Field is `null` but schema marks it optional | Delete the key entirely | `{ path: "/tmp", offset: null }` → `{ path: "/tmp" }` |
| `bare-string-wrap` | Bare string where array expected | Wrap in `[ ]` | `"foo"` → `["foo"]` |
| `empty-placeholder-fix` | Object `{}` where array expected | Convert to `[{}]` or inspect schema; default to `[]` if schema allows empty | `{}` → `[]` (context‑dependent) |

**Execution order is critical:** `json-array-parse` before `bare-string-wrap`, else `'["a"]'` is incorrectly wrapped to `['["a"]']`.

**2.1.2 Markdown Auto‑Link Unwrap**

DeepSeek frequently emits file paths as markdown links (`[notes.md](http://notes.md)`) due to its chat fine‑tuning.

- **Detection:** Field name or annotation matches `*path*` or uses custom `pathString()` validator.
- **Repair:** Apply regex to extract raw path only when the link text equals the URL minus protocol (i.e., degenerate auto‑link). Real markdown links with distinct text and full URLs pass through unchanged.
- **Integration:** This is not a generic repair; it is attached to schema‑level metadata (`format: "path"`).

**2.1.3 Relational Invariant Injection**

Some constraints span multiple fields (e.g., `offset` requires `limit`). This cannot be fixed by single‑field repair.

- **Implementation:** On validation failure for a relational invariant, the harness *defaults* the missing counterpart and adds a transparent note to the tool result.
- **Convention:** Use non‑error prefix (e.g., `Note:`) so the UI does not flag it as a failure.
- **Example:**  
  - `readFile({ absolutePath: ..., limit: 30 })` → harness inserts `offset: 0` and appends `Note: offset defaulted to 0. To change, retry with both offset and limit.`

### 2.2 Streaming & Structural Repair (Output‑Side)

DeepSeek’s tool calls often break across streaming chunks.

- **Reassembly Buffer:** Accumulate partial tool‑call deltas until a complete, valid JSON object is formed. Implement a timeout (e.g., 2 seconds) after which a repair attempt is made, or an error is returned to the model.
- **Orphaned Tool Call Recovery:** A state‑ful middleware scans the conversation history before each new turn. If any `ToolCall` lacks a corresponding `ToolResult`, it injects a synthetic result (`{ "error": "Tool execution interrupted; please retry." }`), preventing hard crashes.

### 2.3 Schema‑Aware Contract Hints

The harness should let developers annotate tool schemas with hints that guide both the model and the repair layer.

**2.3.1 Custom Validators with Semantic Tags**

- `pathString()` – Marks a string as a filesystem path. Triggers auto‑link unwrap, tilde expansion (`~`), and relative‑to‑absolute resolution.
- `nonEmptyArray()` – Explicitly disallows `null`, `{}`, and bare strings; the repair layer uses this tag to choose the correct fix.
- `relational({ requires: ['offset', 'limit'] })` – Declares relational constraints so they can be enforced and defaulted generically.

**2.3.2 Tool Description Language**

Automatically append a brief “contract expectation” to the tool description sent to the model. For example:
> `writeFile` – `content` must be a plain string, not a JSON object. `filePath` is an absolute filesystem path, **not** a markdown link.

This gives the model a local cue to suppress its chat distribution without bloating the system prompt.

### 2.4 Intelligent Context Management

**2.4.1 Dynamic Tool Search**

With many tools, full definitions bloat context. Provide a meta‑tool `tool_search(query: string)` that returns a shortlist of relevant tools. The main model calls this first, and the harness injects only those definitions into the next turn.

**2.4.2 Context Partitioning**

For large tool outputs (logs, file contents), store the full output on disk and return a summary with a hyper‑reference.

> *“Test run completed. 3 failures in 1543 lines. Full output stored at `/tmp/run.log`. To inspect, call `read_file` with an offset/limit.”*

This prevents the model from being overwhelmed and gives it agency to fetch details on demand.

### 2.5 Agent‑Level Control & Safety

**2.5.1 Retry‑with‑Feedback Loop**

When a tool call fails after repair, the harness **must not** silently fallback. It formats a model‑readable error (JSON with `error` key, or a specific `ToolResult` field) and feeds it back as a new assistant turn. This lets the model self‑correct in the same conversation.

- **Implementation:** A configurable `maxRetries` per tool (default 1–2). The error message includes the original invalid parameters and the reason, e.g.:  
  `"review_code failed: 'commit_id' is required but was missing. Please provide a valid commit_id."`

**2.5.2 Runaway Agent Guard (Tool‑Loop Circuit Breaker)**

Detect infinite loops by counting consecutive identical tool calls (same tool + same arguments). After a threshold (e.g., 5), inject a system message: *“You have repeated the same action. Please summarise your findings and propose a new approach.”* If the agent continues, terminate the turn with a forced error.

**2.5.3 Programmatic Tool Calling (Optional Advanced Feature)**

Allow the model to write and execute a short script (e.g., Python) that orchestrates multiple tool calls in a single turn. This is not a priority for DeepSeek stability but can dramatically reduce latency and token cost once the basic harness is mature.

### 2.6 Observability & Telemetry

Every harness decision must be measurable.

- **Repair Logging:** Emit `tool_input_repaired:{toolName}` and the specific repair(s) applied.
- **Failure Logging:** `tool_input_invalid:{toolName}` when no repair works, with the raw input for offline analysis.
- **Per‑Model Dashboards:** Track repair rates per (model, tool, repair type) to detect regressions (e.g., a fine‑tuned model suddenly starts producing more bare strings).
- **Link to CI:** Fail the build if repair rate for a critical tool exceeds a threshold.

---

## 3. Implementation Roadmap

### Phase 1: Core Resilience (Week 1‑2)
- Build the `validate‑then‑repair` pipeline with the four compositional repairs.
- Implement markdown auto‑link unwrap integrated with `pathString()`.
- Implement the streaming reassembly buffer and orphaned result injection.
- Basic retry‑with‑feedback (single retry on any tool failure).
- Rudimentary logging to console / file.

**Deliverable:** DeepSeek Flash can reliably call `readFile`, `writeFile`, `shellCommand` without fatal errors.

### Phase 2: Schema Intelligence & Context (Week 3‑4)
- Add custom validators (`pathString`, `nonEmptyArray`, relational constraints).
- Implement relational default injection (offset/limit pattern).
- Dynamic tool search meta‑tool.
- Context partitioning with disk‑backed large outputs.
- Create a developer guide for annotating tool schemas.

**Deliverable:** DeepSeek V4 Pro matches or exceeds commercial model reliability on internal coding evals.

### Phase 3: Agent Safety & Advanced Flows (Week 5‑6)
- Tool‑loop circuit breaker.
- Per‑tool configurable retry limits and custom error messages.
- Telemetry pipeline (structured logs → dashboard).
- Optionally, programmatic tool calling if resource permits.

### Phase 4: Hardening & Open Source (Ongoing)
- Collect anonymised failure logs and expand the repair catalogue.
- Fuzz‑test the harness against DeepSeek and other open models.
- Publish the harness as a standalone library with adapters for popular agent frameworks (LangChain, CrewAI, custom).

---

## 4. Technical Decisions & Guardrails

- **Input immutability for valid data:** If the first parse succeeds, the original input is used as‑is. No normalisation, no silent changes. This prevents corruption of file contents that happen to look like JSON.
- **Repair budget:** A single repair pass per tool call; if it still fails, immediately log and return the error. No iterative guessing games.
- **Model‑agnostic but DeepSeek‑informed:** All repairs are general patterns, but the catalogue and order were derived from DeepSeek’s behaviour. Other open models will benefit similarly.
- **Transparency over magic:** Every repair and default is surfaced back to the model in the tool result so it can adapt its subsequent actions. This builds trust and prevents hidden state drift.

---

## 5. Success Metrics

- **Repair Rate > 95%** for DeepSeek Flash on canonical coding tasks (file ops, shell commands, code review) without a single fatal tool error.
- **Zero manual intervention** in 100‑consecutive‑turn agent runs.
- **Reduction in token waste** by 30% via tool search and context partitioning (compared to full‑definition dumping).
- **Time to diagnose a new model failure** reduced from hours to minutes using repair logs.

---

Phase 1 Core Rust Interception Layer

The first phase requires building the interception layer directly into the Rust engine before formal deserialization occurs. You will need to capture the raw string payload from the DeepSeek output and pass it through a dedicated repair module instead of handing it straight to Serde. This module will apply the compositional repairs in strict order starting with JSON array parsing and moving to bare string wrapping. Once the payload successfully deserializes into a valid Rust struct you can immediately pipe any code related outputs through Tree-sitter. This guarantees that any generated code is structurally sound before it ever reaches the filesystem or executing environment.
Phase 2 Janet Schema and Protocol Alignment

The next step involves formalizing the tool definitions within the Janet plugin ecosystem using the Model Context Protocol. You will write Janet macros or functions that define the tool capabilities and custom validators. The Rust host will evaluate these Janet scripts at startup to dynamically generate the exact JSON schema required by the model. This is where you implement the markdown path unwrap logic by flagging specific Janet fields as paths. The Rust layer can then apply a regex pass on those specific fields to strip out chat interface formatting before passing the clean arguments back to the Janet execution context.
Phase 3 Prompt Topography and Context Partitioning

With the basic execution loop stabilized you must optimize the context window for DeepSeek cache performance. You will rewrite the prompt generation logic to strictly place system instructions and the generated tool schemas at the absolute beginning of the payload. The dynamic elements like the precomputed environment tree will be appended next to eliminate the orientation phase on the first turn. You will also need to add a streaming utility in Rust that intercepts massive outputs from Janet shell executions. This utility will write the bulk data to a temporary file and return a concise summary to the model to preserve token space and keep the context window clean.
Phase 4 Dynamic Tiering and Cognitive Routing

The final phase turns the harness into a truly cognitive router. You will implement a dual client setup in Rust that defaults to DeepSeek Flash for the standard validate and execute loop. You will track the failure state of each tool call and trigger an escalation path to the Pro tier whenever the repair layer exhausts its options or Tree-sitter detects a deep semantic error. To prevent long running agents from losing the plot you will add a context depth counter. When the execution tree grows too deep the engine will automatically inject targeted reminders about relational constraints into the immediate prompt space to keep the model grounded.