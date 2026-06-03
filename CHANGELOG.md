# Changelog

All notable changes to dirge are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.4] - 2026-06-03

### Added
- **Debug Adapter Protocol (DAP) integration**: step-through debugging
  alongside the existing LSP client, hardened against the usual adapter
  failure modes (UB, hangs, panics).
- **Configurable terminal background**: themes can set a `background` color,
  and the built-in phosphor theme defaults to a soft charcoal `#222222`. The
  plain theme keeps the terminal's own background (`Reset`).
- **Snap-to-bottom on input**: typing in the prompt or pressing Down on a
  scrolled-up chat jumps straight back to the latest output instead of
  requiring a manual scroll.
- **Phased plan workflow** (`/plan <request>`, opt-in via
  `phased_workflow_enabled`): an explicit per-task command that runs
  explore → plan → implement → reviewer-runs-code loop. The explore and
  plan phases are context-isolated read-only forks; the implement phase
  is a normal streamed turn; a write-disabled reviewer fork then runs the
  code and emits a machine-parsed verdict, with `NEEDS_FIX` feeding a
  punch-list back for a bounded re-implement (`phased_workflow_max_review_cycles`,
  default 2). Ported from [vix](https://github.com/kirby88/vix). See
  [docs/agent-loop.md](docs/agent-loop.md#phased-plan-workflow-plan).
- **Minified tree-sitter read/edit** (`read_minified` / `edit_minified`):
  token-efficient file I/O that collapses a file to its structural
  skeleton — aggressive collapse for Rust/Java/Go, gap-preserving collapse
  for whitespace/ASI-sensitive grammars (Bash, Python, Ruby, Elixir, C/C++,
  TS, Clojure). Each gated on its `semantic-<lang>` feature.
- **Hard read-before-edit gate**: `edit`/`apply_patch` to a file never
  read this session is refused mechanically.
- **Thinking-stall watchdog**: the request-timeout backstop now injects a
  summary-reinjection nudge for graceful recovery from a stalled run.
- **Mandatory reason/intent fields** on the read/grep/glob/find/lsp tools
  (and bash anti-misuse fields), plus a **todo-completion nudge** that
  blocks a premature `end_turn` while todo items remain pending.
- Config keys `phased_workflow_enabled`, `phased_workflow_max_review_cycles`,
  and documentation for the pre-existing `dynamic_tool_search` and
  `context_depth_reminder_threshold` keys.

### Fixed
- **/plan busy indicator**: `/plan` now shows the standard busy state and
  clears the submitted text from the input box while the explore/plan forks
  run (previously it looked idle with the command still lingering), and the
  busy flag can no longer be stranded "running" by a failed repaint.
- **Resize scroll clamp**: enlarging the terminal while scrolled up no longer
  leaves a stale scroll offset that hid the newest output behind blank rows.
- **Idle Ctrl+C** now clears a typed-but-unsent draft instead of quitting
  outright; only an empty input line exits.
- **Home/End** moved to **Ctrl+Home/End** for chat scroll, freeing bare
  Home/End for the input editor's line-start/line-end as documented.
- Ctrl+J inside reverse-i-search no longer desyncs the search buffer.

### Acknowledgements
- Added [vix](https://github.com/kirby88/vix) — the battle-tested Go coding
  agent the above agentic-loop features were ported from.

## [0.2.3] - 2026-06-02

### Added
- Unified permission/authorization engine (single Policy Decision Point):
  op-based rules, `/why` decision-trace command, atomic multi-claim bash.
- Input box scrolls to keep the cursor visible past the height cap, and
  Up/Down navigate across soft-wrapped display rows.
- Cohesive low-saturation phosphor palette (hue = action, brightness =
  importance), a dedicated soft "thinking" color, and syntax-highlighted
  `read` boxes. Critic/thinking colors are config-themeable.
- Config-driven plugin toggles (`plugins.<name>.{enabled, auto_start}`)
  and a bundled `backpressured` validation-gated loop plugin.

### Changed
- Lighter terminal UI: the heavy double-line frame is now light
  single-line/rounded, the side panels follow the main frame's theme
  color, and the input prompt is a simple `> `.
- Reasoning/thinking is suppressed by default (spinner + Ctrl+O to
  expand) to keep the conversation focused.

### Fixed
- Secrets in tool output are redacted before reaching the LLM / session
  transcript.
- Transient LLM connection failures ("error sending request") now retry
  with exponential backoff.
- Questionnaire custom answers soft-wrap instead of running off-screen.
- Edit results collapse the appended LSP diagnostics into a one-line
  summary (Ctrl+O to expand); diagnostic floods are summarized and the
  per-file cap tightened, so an unsupported language server no longer
  floods the chat.
- A configured `deny` rule is now terminal above a session allowlist.
- Resumed sessions keep persisting after a save (loaded-mtime refresh).

### Packaging
- Published to crates.io as the **`dirge-agent`** crate (the short
  `dirge` name was taken); the installed binary is still `dirge`:
  `cargo install dirge-agent`.

## [1.0.0]

First tagged release. dirge is a minimalistic, memory-efficient coding
agent in Rust with:

- A terminal UI with markdown rendering, scrollback, and an info panel.
- Configurable permission modes (standard / restrictive / accept / yolo)
  with op-based rules and session allowlists.
- Tree-sitter bash permission parsing and semantic code tools for
  TypeScript, Python, Clojure, Go, Ruby, Rust, Java, C, and C++.
- Claude-compatible skills, persistent project memory, subagents, MCP and
  LSP integration, and a Janet plugin system.
- Session save/load/resume with LLM-summarization compaction.

[Unreleased]: https://github.com/dirge-code/dirge/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/dirge-code/dirge/releases/tag/v1.0.0
