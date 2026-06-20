# Plugin-registered keybinding overrides (#476).
#
# `harness/bind-key keys command` remaps a BUILT-IN command — unlike
# `register-shortcut`, which binds a key to plugin code. `command` is any
# name from the global or input-editor command tables (see docs/config.md
# "Key bindings"), or "none" to unbind a default. `keys` may be a single
# chord or an emacs-style sequence like "ctrl-x ctrl-s".
#
# Precedence: built-in defaults < these plugin bindings < the user's
# `keybindings` config. The user always wins, so anything here is just a
# nicer default they can still override.
#
# Reserved keys neither form can override: Ctrl+C, Ctrl+D, Esc (the panic
# gesture) and the search / rewind picker keys.

# Remap chat scrolling to an emacs sequence.
(harness/bind-key "ctrl-x ctrl-t" "scroll_to_top")
(harness/bind-key "ctrl-x ctrl-b" "scroll_to_bottom")

# Add an alternate chord for an input-editor command.
(harness/bind-key "alt-a" "cursor_line_start")

# Disable a default you don't want.
(harness/bind-key "ctrl-r" "none")
