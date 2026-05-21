use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, Query, QueryCursor};

use crate::semantic::adapter::LanguageAdapter;
use crate::semantic::types::{ByteRange, ExtractedFile, Import, Symbol, SymbolKind};

/// Tree-sitter adapter for Rust. dirge itself is written in Rust;
/// this was a glaring gap — list_symbols / find_callers worked for
/// Python, TS, and Clojure but not for the codebase the agent is
/// most often editing.
///
/// Exports are detected via `visibility_modifier` ("pub", "pub(crate)",
/// "pub(super)", etc.). Anything visibility-tagged counts as exported;
/// items without a visibility modifier stay private.
pub struct RustAdapter;

impl RustAdapter {
    fn text<'a>(&self, n: Node<'a>, s: &'a [u8]) -> &'a str {
        n.utf8_text(s).unwrap_or("")
    }

    fn range(&self, n: Node) -> ByteRange {
        ByteRange {
            start_byte: n.start_byte(),
            end_byte: n.end_byte(),
            start_line: n.start_position().row + 1,
            end_line: n.end_position().row + 1,
        }
    }

    fn signature(&self, n: Node, s: &[u8]) -> String {
        // Function signature is everything up to the body's `{`.
        if let Some(body) = n.child_by_field_name("body") {
            return String::from_utf8_lossy(&s[n.start_byte()..body.start_byte()])
                .trim()
                .to_string();
        }
        // Fall back to first line capped at 80.
        let first = self.text(n, s).lines().next().unwrap_or("");
        if first.chars().count() > 80 {
            let p: String = first.chars().take(80).collect();
            format!("{p}…")
        } else {
            first.to_string()
        }
    }

    /// True if any direct child is a `visibility_modifier`.
    fn is_exported(&self, n: Node) -> bool {
        for i in 0..n.named_child_count() {
            if let Some(c) = n.named_child(i)
                && c.kind() == "visibility_modifier"
            {
                return true;
            }
        }
        false
    }

    fn ident_child<'a>(&self, n: Node<'a>, s: &'a [u8]) -> Option<String> {
        // `function_item` uses `identifier`; `struct_item`/`enum_item`/
        // `trait_item`/`type_item` use `type_identifier`. Try both.
        for i in 0..n.named_child_count() {
            let c = n.named_child(i)?;
            if matches!(c.kind(), "identifier" | "type_identifier") {
                return Some(self.text(c, s).to_string());
            }
        }
        None
    }

    fn handle_function(&self, n: Node, s: &[u8], symbols: &mut Vec<Symbol>) {
        let Some(name) = self.ident_child(n, s) else {
            return;
        };
        symbols.push(Symbol {
            kind: SymbolKind::Function,
            is_exported: self.is_exported(n),
            name,
            range: self.range(n),
            signature: self.signature(n, s),
            parent_class: None,
        });
    }

    fn handle_struct_or_enum(&self, n: Node, s: &[u8], symbols: &mut Vec<Symbol>) {
        let Some(name) = self.ident_child(n, s) else {
            return;
        };
        symbols.push(Symbol {
            kind: SymbolKind::Class,
            is_exported: self.is_exported(n),
            name,
            range: self.range(n),
            signature: self.text(n, s).lines().next().unwrap_or("").to_string(),
            parent_class: None,
        });
    }

    fn handle_trait(&self, n: Node, s: &[u8], symbols: &mut Vec<Symbol>) {
        let Some(trait_name) = self.ident_child(n, s) else {
            return;
        };
        symbols.push(Symbol {
            kind: SymbolKind::Interface,
            is_exported: self.is_exported(n),
            name: trait_name.clone(),
            range: self.range(n),
            signature: self.text(n, s).lines().next().unwrap_or("").to_string(),
            parent_class: None,
        });
        // Walk trait body for required-method signatures + provided
        // method bodies; both become Method symbols anchored to the
        // trait name.
        for i in 0..n.named_child_count() {
            let Some(c) = n.named_child(i) else { continue };
            if c.kind() != "declaration_list" {
                continue;
            }
            for j in 0..c.named_child_count() {
                let Some(m) = c.named_child(j) else { continue };
                let mname = match m.kind() {
                    "function_item" | "function_signature_item" => self.ident_child(m, s),
                    _ => None,
                };
                if let Some(mname) = mname {
                    symbols.push(Symbol {
                        kind: SymbolKind::Method,
                        is_exported: true,
                        name: mname,
                        range: self.range(m),
                        signature: self.signature(m, s),
                        parent_class: Some(trait_name.clone()),
                    });
                }
            }
        }
    }

    fn handle_impl(&self, n: Node, s: &[u8], symbols: &mut Vec<Symbol>) {
        // `impl Type { ... }` or `impl Trait for Type { ... }`. The
        // RECEIVING type is the most useful parent_class — it's what
        // the user types when they want "all methods on Foo".
        let mut last_type: Option<String> = None;
        for i in 0..n.named_child_count() {
            if let Some(c) = n.named_child(i)
                && c.kind() == "type_identifier"
            {
                last_type = Some(self.text(c, s).to_string());
            }
        }
        let Some(receiving) = last_type else {
            return;
        };
        for i in 0..n.named_child_count() {
            let Some(c) = n.named_child(i) else { continue };
            if c.kind() != "declaration_list" {
                continue;
            }
            for j in 0..c.named_child_count() {
                let Some(m) = c.named_child(j) else { continue };
                if m.kind() != "function_item" {
                    continue;
                }
                if let Some(mname) = self.ident_child(m, s) {
                    symbols.push(Symbol {
                        kind: SymbolKind::Method,
                        is_exported: self.is_exported(m),
                        name: mname,
                        range: self.range(m),
                        signature: self.signature(m, s),
                        parent_class: Some(receiving.clone()),
                    });
                }
            }
        }
    }

    fn handle_type_alias(&self, n: Node, s: &[u8], symbols: &mut Vec<Symbol>) {
        let Some(name) = self.ident_child(n, s) else {
            return;
        };
        symbols.push(Symbol {
            kind: SymbolKind::TypeAlias,
            is_exported: self.is_exported(n),
            name,
            range: self.range(n),
            signature: self.text(n, s).lines().next().unwrap_or("").to_string(),
            parent_class: None,
        });
    }

    fn handle_const_or_static(&self, n: Node, s: &[u8], symbols: &mut Vec<Symbol>) {
        // `const NAME: T = ...;` / `static NAME: T = ...;` — the
        // name is an `identifier` child.
        let mut name: Option<String> = None;
        for i in 0..n.named_child_count() {
            if let Some(c) = n.named_child(i)
                && c.kind() == "identifier"
            {
                name = Some(self.text(c, s).to_string());
                break;
            }
        }
        let Some(name) = name else { return };
        symbols.push(Symbol {
            kind: SymbolKind::Variable,
            is_exported: self.is_exported(n),
            name,
            range: self.range(n),
            signature: self.text(n, s).lines().next().unwrap_or("").to_string(),
            parent_class: None,
        });
    }

    fn handle_use(&self, n: Node, s: &[u8], imports: &mut Vec<Import>) {
        // `use std::sync::Arc;` — the first non-keyword child is
        // the path. Render it as a single import string; opencode/pi
        // do similar.
        for i in 0..n.named_child_count() {
            let Some(c) = n.named_child(i) else { continue };
            match c.kind() {
                "scoped_identifier" | "identifier" | "use_list" | "use_as_clause" => {
                    let path = self.text(c, s).to_string();
                    imports.push(Import {
                        names: vec![path.clone()],
                        source: path,
                    });
                    break;
                }
                _ => {}
            }
        }
    }

    fn find_node_at_range<'a>(&self, n: Node<'a>, start: usize, end: usize) -> Option<Node<'a>> {
        if n.start_byte() == start && n.end_byte() == end {
            return Some(n);
        }
        for i in 0..n.named_child_count() {
            if let Some(c) = n.named_child(i)
                && c.start_byte() <= start
                && c.end_byte() >= end
                && let Some(f) = self.find_node_at_range(c, start, end)
            {
                return Some(f);
            }
        }
        None
    }
}

impl LanguageAdapter for RustAdapter {
    fn extensions(&self) -> &[&str] {
        &[".rs"]
    }

    fn extract(&self, file_path: &Path, source: &str) -> Result<ExtractedFile, String> {
        let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let mut parser = Parser::new();
        parser
            .set_language(&lang)
            .map_err(|e| format!("Failed to set language: {e}"))?;
        let tree = parser.parse(source, None).ok_or("Failed to parse source")?;
        let root = tree.root_node();
        let bytes = source.as_bytes();

        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let exports = Vec::new();
        let mut warnings = Vec::new();

        if root.has_error() {
            warnings.push("tree-sitter reported syntax errors".to_string());
        }

        for i in 0..root.named_child_count() {
            let Some(c) = root.named_child(i) else {
                continue;
            };
            match c.kind() {
                "function_item" => self.handle_function(c, bytes, &mut symbols),
                "struct_item" | "enum_item" | "union_item" => {
                    self.handle_struct_or_enum(c, bytes, &mut symbols);
                }
                "trait_item" => self.handle_trait(c, bytes, &mut symbols),
                "impl_item" => self.handle_impl(c, bytes, &mut symbols),
                "type_item" => self.handle_type_alias(c, bytes, &mut symbols),
                "const_item" | "static_item" => self.handle_const_or_static(c, bytes, &mut symbols),
                "use_declaration" => self.handle_use(c, bytes, &mut imports),
                _ => {}
            }
        }

        Ok(ExtractedFile {
            file_path: file_path.to_path_buf(),
            symbols,
            imports,
            exports,
            warnings,
            mtime: std::time::SystemTime::now(),
        })
    }

    fn find_callees_in_range(
        &self,
        source: &str,
        _file_path: &Path,
        range: ByteRange,
    ) -> Result<Vec<String>, String> {
        let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let mut parser = Parser::new();
        parser
            .set_language(&lang)
            .map_err(|e| format!("Failed to set language: {e}"))?;
        let tree = parser.parse(source, None).ok_or("Failed to parse source")?;
        let root = tree.root_node();
        let bytes = source.as_bytes();

        let target = self
            .find_node_at_range(root, range.start_byte, range.end_byte)
            .ok_or("Could not find node at given range")?;

        // Direct call: `foo(...)`. Method call: `obj.bar(...)` —
        // tree-sitter-rust models the call as `call_expression` with
        // function = `field_expression`; we capture the field name.
        // Macro invocations (`println!`, etc.) appear separately as
        // `macro_invocation`; capture their identifier too.
        let query_str = r#"
            (call_expression function: (identifier) @callee)
            (call_expression function: (field_expression field: (field_identifier) @callee))
            (macro_invocation macro: (identifier) @callee)
        "#;
        let query = Query::new(&lang, query_str).map_err(|e| format!("Query error: {e}"))?;
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, target, bytes);

        let mut callees = Vec::new();
        while let Some(m) = matches.next() {
            for capture in m.captures {
                let name = capture.node.utf8_text(bytes).unwrap_or("");
                callees.push(name.to_string());
            }
        }
        callees.sort();
        callees.dedup();
        Ok(callees)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb(n: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(n)
    }

    #[test]
    fn extracts_pub_fn_as_exported_and_private_fn_not() {
        let src = "pub fn a() {}\nfn b() {}\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        let a = f.symbols.iter().find(|s| s.name == "a").unwrap();
        let b = f.symbols.iter().find(|s| s.name == "b").unwrap();
        assert!(a.is_exported);
        assert!(!b.is_exported);
        assert!(matches!(a.kind, SymbolKind::Function));
    }

    #[test]
    fn extracts_struct_enum_as_class() {
        let src = "pub struct Foo { name: String }\npub enum Bar { A, B }\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        assert!(
            f.symbols
                .iter()
                .any(|s| s.name == "Foo" && matches!(s.kind, SymbolKind::Class))
        );
        assert!(
            f.symbols
                .iter()
                .any(|s| s.name == "Bar" && matches!(s.kind, SymbolKind::Class))
        );
    }

    #[test]
    fn extracts_trait_with_method_signatures() {
        let src = "pub trait Greeter {\n  fn greet(&self) -> String;\n  fn default_greet(&self) -> String { \"hi\".to_string() }\n}\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        let trait_sym = f.symbols.iter().find(|s| s.name == "Greeter").unwrap();
        assert!(matches!(trait_sym.kind, SymbolKind::Interface));
        let g = f.symbols.iter().find(|s| s.name == "greet").unwrap();
        assert_eq!(g.parent_class.as_deref(), Some("Greeter"));
        let dg = f
            .symbols
            .iter()
            .find(|s| s.name == "default_greet")
            .unwrap();
        assert_eq!(dg.parent_class.as_deref(), Some("Greeter"));
    }

    #[test]
    fn impl_methods_attach_to_receiving_type() {
        let src = "pub struct Foo;\nimpl Greeter for Foo {\n  fn greet(&self) -> String { String::new() }\n}\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        let g = f.symbols.iter().find(|s| s.name == "greet").unwrap();
        assert!(matches!(g.kind, SymbolKind::Method));
        // For `impl Trait for Type`, the receiving type (Foo) is the
        // last type_identifier; that's what list_symbols filter on
        // `--parent Foo` should match.
        assert_eq!(g.parent_class.as_deref(), Some("Foo"));
    }

    #[test]
    fn extracts_type_alias() {
        let src = "pub type Id = u64;\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        let id = f.symbols.iter().find(|s| s.name == "Id").unwrap();
        assert!(matches!(id.kind, SymbolKind::TypeAlias));
        assert!(id.is_exported);
    }

    #[test]
    fn extracts_const_and_static_as_variable() {
        let src = "pub const MAX: u32 = 42;\nstatic GLOBAL: i32 = 0;\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        let m = f.symbols.iter().find(|s| s.name == "MAX").unwrap();
        let g = f.symbols.iter().find(|s| s.name == "GLOBAL").unwrap();
        assert!(matches!(m.kind, SymbolKind::Variable));
        assert!(m.is_exported);
        assert!(!g.is_exported);
    }

    #[test]
    fn extracts_use_imports() {
        let src = "use std::sync::Arc;\nuse crate::foo::Bar;\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        assert!(
            f.imports
                .iter()
                .any(|i| i.source.contains("std::sync::Arc"))
        );
        assert!(
            f.imports
                .iter()
                .any(|i| i.source.contains("crate::foo::Bar"))
        );
    }

    #[test]
    fn find_callees_captures_direct_method_and_macro() {
        let src = "pub fn run() { helper(); foo.bar(); println!(\"x\"); }\nfn helper() {}\n";
        let f = RustAdapter.extract(&pb("x.rs"), src).unwrap();
        let run = f.symbols.iter().find(|s| s.name == "run").unwrap();
        let callees = RustAdapter
            .find_callees_in_range(src, &pb("x.rs"), run.range)
            .unwrap();
        assert!(callees.contains(&"helper".to_string()));
        assert!(callees.contains(&"bar".to_string()));
        assert!(callees.contains(&"println".to_string()));
    }
}
