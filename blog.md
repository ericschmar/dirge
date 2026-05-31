# dirge: the coding agent that fits in your pocket and punches above its weight

Most coding agents are resource hogs. The market leader clocks in at ~300MB RAM just sitting idle. Dirge is written in Rust, ships in a ~30MB binary, and runs at **~8MB idle, ~15MB working**. You could run twenty copies of it and still use less memory than a single instance of the popular alternatives.

But the lean footprint is table stakes. What makes dirge worth a serious look is a different bet about where agent quality comes from.

Most agentic harnesses are thin: a model, a tool loop, a system prompt, and an instruction to get out of the way. All the intelligence is assumed to live in the model, and the harness is plumbing. Dirge takes the opposite position — that a large fraction of real-world agent performance lives in the *harness*, and that an agent which invests there can make a cheaper or open model behave like a far more expensive one.

That bet shows up in three places no other agent combines: a genuinely pluggable embedded scripting system inspired by Pi, a harness that actively steers, repairs, and verifies what the model does, and a per-project learning architecture that gets smarter every time you use it.

## Janet: the plugin system coding agents actually need

Most agents aren't very customizable. You typically get one of two things. Either a config file with a handful of flags and callbacks — fine for toggling behavior, useless for building new behavior. Or you get MCP: a plugin model where every extension is a separate process behind a JSON-RPC transport, discovered and invoked through the same LLM that's already trying to solve your actual problem. MCP is great for connecting to external systems; it's a heavy and indirect way to change how the agent itself works.

[Pi](https://pi.dev/) is the rare exception that took extensibility seriously, and it got the core idea right: expose the *agent lifecycle* as hooks, hand users a real programming language, and they will build things you never anticipated — workflows, guardrails, integrations — without forking the agent. Dirge models its plugin system on that philosophy.

It embeds [Janet](https://janet-lang.org) — a small, fast, embeddable Lisp — directly into the agent process. Plugins are `.janet` files you drop into `~/.config/dirge/plugins/` or `./.dirge/plugins/`. They run on a dedicated worker thread, separate from the agent loop and the UI, so a misbehaving plugin can't starve your session.

Why Janet? The whole language fits in about 1MB and embeds with no dynamic linking and negligible startup cost. It's S-expressions all the way down, so plugin hooks receive and return structured data that looks exactly like the code you write — no impedance mismatch between config and logic. And it's a real language (PEG parsing, fibers, destructuring), not a watered-down DSL.

A minimal plugin looks like this:

```janet
(defn on-prompt [ctx]
  (when (string/find "security" (ctx :prompt))
    (harness/notify "running with security mindset" :info)))
```

But the full harness API is a proper operating surface. Plugins can intercept any tool call to block, rewrite its input, or replace its result; register slash commands and first-class custom tools the LLM sees natively; register custom LLM providers pointing at a local endpoint; control the session tree (fork, label, navigate); and open blocking dialogs that pause the worker until the user answers — synchronous, not callback spaghetti. Eleven lifecycle hooks (`on-init`, `on-prompt`, `on-tool-start`, `on-tool-end`, `on-error`, `prepare-next-run`, and more) let a multi-file plugin orchestrate real workflows.

The example plugins show the range, each in under 100 lines of Janet: an architect → implementor → reviewer workflow via inversion of control; path protection; destructive-command confirmation; persona selection; local provider registration. One implements [PlanSearch](https://arxiv.org/abs/2409.03733) — a `/plan` command that forces the model to generate several genuinely different natural-language approaches *before* writing any code, then implement the best one — turning a research result into a one-file plugin.

This is the thing Pi got right about extensibility, and dirge embraces it: give people the lifecycle and a real language, and the agent stops being a fixed product and becomes a platform.

## A harness that makes models perform

You've probably heard that open models like DeepSeek are bad at tool calling — that reliable tool use means paying for a frontier model that memorized every API contract during pretraining.

After a lot of time watching how these models actually interact with a harness, I've reached a different conclusion: most of what looks like a *capability* gap is a *harness* gap. The model usually knows what it wants to do. The harness just has to meet it where it is — guide it before it acts, repair the small mistakes it makes, give it actionable feedback when something's wrong, and verify before it declares victory. Frontier labs bake a lot of this into post-training. Dirge makes it explicit, in the loop, for any model.

**Steering and reasoning guidance.** Dirge ships a research-backed, model-agnostic guidance suite that's baked into the system prompt and the loop — no config required. It enforces a finish discipline (a pre-reply self-check with an explicit definition of done), nudges up-front planning and terse progress notes on multi-step work, and calibrates when to ask versus proceed (ask only on costly, genuinely divergent ambiguity; otherwise act and state the assumption). At the start of each task it retrieves a few *on-topic* worked tool-call demonstrations by lexical match and injects them ahead of the prompt — in-context examples are one of the biggest reliability levers for weaker models. On top of that, model-aware steering tailors the guidance to the active model family, with a DeepSeek-specific fragment for its known quirks.

**Tool-call repair.** A small set of malformed-tool-call shapes accounts for the large majority of "this model can't do tools" complaints: a `null` where a field should be omitted, a JSON array sent as a string, a bare string where an array was wanted, a file path wrapped in a markdown link. Dirge fixes these *in place* — but the order matters: it tries the input as-is first and only repairs the exact paths the schema actually rejected, so valid inputs are never rewritten (the silent-corruption trap that catches naive preprocess-then-validate designs). It also borrows structural ideas from DeepSeek-tuned agent loops: flattening nested tool schemas the model handles poorly, and scavenging tool calls the model described in its reasoning text but failed to emit in the structured field.

**Syntax feedback before bytes hit disk.** This one is genuinely uncommon. Every `write`, `edit`, and `apply_patch` is parsed through the matching tree-sitter grammar *before* the file is saved. Syntactically broken code is rejected with line- and column-precise errors — and the error is mechanically enriched so the model never has to count delimiters by hand: the missing token is named straight from the grammar ("missing `}`"), and for brace and Lisp languages a comment/string/char-literal-aware balance summary points at the exact unclosed `(`, `{`, or `[`. The model sees the real problem and corrects it on the *same* turn instead of trusting its own broken output. (This is also why "the agent said it edited the file but nothing changed" stops happening — broken writes are caught, not silently trusted.)

**Verification, not vibes.** A cheap, signal-based in-loop critic watches whether code was edited and whether a build or test actually ran and passed — read straight from exit codes, no semantics parsed. At the moment the agent is about to declare it's done, it injects one soft nudge: *fix it* if a build/test failed after a code change, *verify it* if code changed but nothing was run, and stays silent when things ran green. When you configure a separate (cheaper, faster) critic model, the gate can escalate substantive runs to one bounded second opinion — "is this actually complete and correct?" — and re-enter the loop if the answer is no. It's off by default and costs nothing until you opt in.

**Refusing to spin.** Weak models love to retry the same failing action. A circuit breaker trips on repeated identical tool calls and forces a *reflect-then-pivot*: the model has to diagnose what it tried, name the wrong assumption, and propose a fundamentally different approach. An in-session reflexion buffer remembers every approach it has already abandoned this run, so it can't quietly cycle back to a dead end. Transient API failures are handled separately, with exponential backoff and jitter so concurrent agents don't retry in lockstep. And if you interject mid-run, your message is wrapped as an override the model recognizes — not just another conversational turn.

The through-line: **the model didn't change.** The contract got more forgiving in exactly the places it needed to be, the feedback got actionable instead of cryptic, and the loop got much harder to derail. That's the harness doing its job.

## The learning loop: an agent that remembers

Most coding agents are amnesiac. Every session starts from scratch. The agent doesn't remember that your project uses `eslint-config-custom` and not `@company/eslint-config`, or that the integration-test mock server needs `--feature=test-utils` to start, or that you spent 45 minutes last week debugging a race in the auth middleware.

Dirge is building a per-project learning architecture, ported from Hermes-agent's memory system and adapted for coding, that changes this. Everything lives in `.dirge/` at your project root, so each project builds its own independent knowledge base — knowledge that's true for your Rust service isn't conflated with your Python one.

The pieces work together. A **memory store** accumulates project facts (build commands, conventions, library quirks) alongside a pitfalls file of things that were tried and failed. A **skill system** captures procedural knowledge — "how to do this class of task in *this* codebase" — that the agent creates and refines through experience. A **session database** with full-text search lets the agent query its own history ("how did we solve the migration issue last month?") with no LLM cost. At session end, a **background review** forks the agent with memory-and-skill tools only and asks it what it learned — discovered build commands, user corrections, skills that turned out wrong — and writes the answers back without disturbing your main session. A periodic **curator** keeps the skill library healthy, consolidating overlaps and archiving stale entries (never deleting them). And when a long session nears the context limit, the middle is compressed by a cheaper model into a structured summary so the thread can continue.

This changes the economics of long-running projects. The first session on a new codebase is exploratory. Without memory, every session after it repeats that exploration. With the learning loop, session two picks up where session one left off, and by session ten the agent knows your project almost as well as you do.

## The rest of the package

Dirge ships with everything you'd expect from a modern coding agent and a few things you wouldn't:

- **Multi-provider** — OpenRouter, OpenAI, Anthropic, Gemini, DeepSeek, GLM, Ollama, plus any OpenAI-compatible endpoint
- **LSP integration** — attaches real language servers (rust-analyzer, typescript, pyright, gopls, clojure-lsp, jdtls, clangd, ruby-lsp) and surfaces compile errors on the same turn the agent writes code
- **Tree-sitter semantic tools** — `list_symbols`, `get_symbol_body`, `find_callers`, `find_callees` across 10 languages, AST-powered with no LSP server, no workspace init, just fast indexed traversal
- **Permission system** — four modes, per-tool glob patterns, session allowlists, and a doom-loop detector
- **Git worktrees** — branch-per-task workflow with `/worktree`, `/wt-merge`, `/wt-exit`
- **Subagents** — the `task` tool spawns isolated research or analysis agents
- **Session tree** — fork, branch, and navigate conversation history
- **Mid-execution interjection** — type while the agent runs to queue a follow-up
- **Inline ASCII avatar** — a 5-cell face that reflects what the agent is doing right now (yes, it's silly; yes, it's genuinely useful)

---

**Dirge is open source (GPL-3.0).** Install with `cargo install dirge`, set your API key, and go. The plugin system, the steering-and-repair harness, and the per-project learning loop are all there when you want them — the default experience is just a fast, capable coding agent.

If you're tired of agents that eat half a gig of RAM before doing anything useful, or you've been burned by open models that "can't do tool calls," or you want an agent that actually gets better the more you use it — give dirge a try.

[https://github.com/dirge-code/dirge](https://github.com/dirge-code/dirge)
