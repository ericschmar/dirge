# Plugin-registered keyboard-shortcut example (P9c).
#
# `harness/register-shortcut keys handler &opt description` binds a key
# combination to a Janet handler in interactive mode. Pi-style API
# mirror — `api.registerShortcut(KeyId, {handler})`.
#
# Key spec grammar (case-insensitive):
#   (modifier "-")* key-name
#
# Modifiers: ctrl, control, alt, meta, shift
# Key names: a single character, f1..f12, or one of:
#   enter, esc, tab, backspace, space, up, down, left, right,
#   home, end, pageup, pagedown, delete, insert
#
# Examples: "ctrl-x", "alt-shift-f", "f5", "ctrl-alt-enter"
#
# Handlers run on the Janet worker thread; they receive the matched key
# spec as a single string argument so one handler can serve multiple
# bindings. Returning a non-nil string surfaces as a chat line.
#
# Reserved keys NOT overridable by plugins:
#   Ctrl+C, Ctrl+D (kill / EOF), Esc (mid-run cancel), the search and
#   rewind picker keys, and a handful of built-in chrome bindings
#   (Ctrl+O expand, Ctrl+X drop interjection, PageUp/Down/Home/End).
# Plugin shortcuts dispatch AFTER those but BEFORE text input.

(defn refresh-handler [key]
  (string "F5 (" key ") pressed — plugin says hi"))

(defn save-handler [key]
  (string "save shortcut fired (" key ")"))

(harness/register-shortcut "f5"     "refresh-handler" "Refresh the chat (demo)")
(harness/register-shortcut "ctrl-s" "save-handler"    "Save (demo)")
