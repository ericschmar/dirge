FTS5 external content tables: `'rebuild'` re-indexes using OLD trigger formula; to change indexed content need DELETE + INSERT SELECT. env::set_var is global/unsafe — tests mutating same key race, fix with static Mutex + EnvGuard RAII that clears on Drop. #![allow(dead_code)] at module level hides real dead code — prefer targeted per-item annotations. Schema migrations: PRAGMA user_version gating, IF NOT EXISTS for FTS triggers, handle "duplicate column name" in ALTER TABLE. Atomic writes: atomic_write_sync returns Result<(), Error> not String, need .map_err.
§
## FTS5 formula migration: 'rebuild' doesn't work
External-content FTS5: `INSERT INTO fts(fts) VALUES('rebuild')` re-indexes using old trigger formula. To change indexed content (e.g. add tool_name to index), DELETE FROM fts then INSERT INTO fts SELECT id, new_formula FROM messages.
§
## #![allow(dead_code)] hides real dead code
Module-level suppression in agent_loop/mod.rs and lsp/mod.rs concealed ~50 genuinely unused items. Removing it revealed the true extent. Prefer targeted per-item annotations — even many are better than module-wide silence.
§
## env::set_var + parallel tests = flaky
`std::env::set_var` is global/unsafe/unsynchronized. Tests mutating same key race. Fix: static Mutex + RAII EnvGuard that clears on Drop (applied in dirge_paths.rs).
