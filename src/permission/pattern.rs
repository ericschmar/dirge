use regex::Regex;

#[derive(Debug, Clone)]
pub struct Pattern {
    regex: Regex,
    #[allow(dead_code)]
    pub original: String,
}

impl Pattern {
    /// Filesystem-style glob: `*` matches one path segment (no `/`); `**`
    /// matches any depth. Use for path tools (`read`, `write`, `edit`,
    /// `list_dir`).
    pub fn new(pattern: &str) -> Self {
        Self::compile(pattern, /* path_style */ true)
    }

    /// Shell-style glob for non-path inputs: `*` matches any chars including
    /// `/`. Use for `bash` command patterns, `grep`/`find_files` patterns,
    /// and other tools where the input isn't a filesystem path.
    ///
    /// Without this, a user pattern like `cd *` (suggested by the harness
    /// for `bash` after the user accepts "allow always") would NOT match
    /// `cd /Users/foo/bar` because `[^/]*` stops at the first slash.
    pub fn new_command(pattern: &str) -> Self {
        Self::compile(pattern, /* path_style */ false)
    }

    fn compile(pattern: &str, path_style: bool) -> Self {
        let expanded = expand_home(pattern);
        let regex_str = glob_to_regex(&expanded, path_style);
        let regex = Regex::new(&regex_str).unwrap_or_else(|_| Regex::new("^$").unwrap());
        Pattern {
            regex,
            original: pattern.to_string(),
        }
    }

    pub fn matches(&self, input: &str) -> bool {
        self.regex.is_match(input)
    }
}

fn expand_home(pattern: &str) -> String {
    if pattern == "~" || pattern == "$HOME" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
        return pattern.to_string();
    }
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
        return pattern.to_string();
    }
    if let Some(rest) = pattern.strip_prefix("$HOME/")
        && let Some(home) = dirs::home_dir()
    {
        return format!("{}/{}", home.to_string_lossy(), rest);
    }
    pattern.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: `cd *` saved via "allow always" must match the user's NEXT
    // `cd /absolute/path` command. The original bug was filesystem-glob
    // semantics applied to a shell-command pattern: `*` compiled to `[^/]*`,
    // refusing to cross slashes. Allowlist entries for bash never fired and
    // the agent re-prompted on every command.
    #[test]
    fn regression_command_pattern_cd_star_matches_path_arg() {
        let pat = Pattern::new_command("cd *");
        assert!(pat.matches("cd /Users/yogthos/src/work/foo"));
        assert!(pat.matches("cd /Users/yogthos/src/work/foo && git diff"));
        assert!(pat.matches("cd foo"));
    }

    #[test]
    fn regression_command_pattern_anchors_to_start() {
        // Don't over-rotate: `cd *` shouldn't match commands that merely
        // contain `cd ` somewhere later.
        let pat = Pattern::new_command("cd *");
        assert!(!pat.matches("xcd foo"));
        assert!(!pat.matches("echo cd foo"));
    }

    #[test]
    fn path_pattern_star_still_excludes_slash() {
        let pat = Pattern::new("src/*");
        assert!(pat.matches("src/main.rs"));
        // Single segment only — `*` doesn't span directory boundaries.
        assert!(!pat.matches("src/agent/main.rs"));
    }

    #[test]
    fn path_pattern_double_star_spans_directories() {
        let pat = Pattern::new("src/**");
        assert!(pat.matches("src/main.rs"));
        assert!(pat.matches("src/agent/main.rs"));
        assert!(pat.matches("src/agent/tools/foo.rs"));
    }

    #[test]
    fn command_pattern_question_mark_matches_any_char() {
        let pat = Pattern::new_command("file.?");
        assert!(pat.matches("file.a"));
        // For commands, `?` is unrestricted.
        assert!(pat.matches("file./"));
    }

    #[test]
    fn path_pattern_question_mark_excludes_slash() {
        let pat = Pattern::new("file.?");
        assert!(pat.matches("file.a"));
        assert!(!pat.matches("file./"));
    }

    #[test]
    fn home_expansion_works_for_both_styles() {
        if let Some(home) = dirs::home_dir() {
            let expected = format!("{}/foo/bar", home.display());
            assert!(Pattern::new("~/foo/*").matches(&expected));
            assert!(Pattern::new_command("~/foo/*").matches(&expected));
        }
    }

    /// F3 (dirge-efw): a trailing ` *` in a command-style pattern
    /// makes the args optional. So `ls *` matches BOTH `ls` (no
    /// args) and `ls -la` (with args). Matches opencode's
    /// `util/wildcard.ts:13-15` semantic. Without this, a session
    /// allowlist entry `ls *` re-prompts when the agent next
    /// invokes bare `ls`.
    #[test]
    fn f3_command_trailing_space_star_makes_args_optional() {
        let pat = Pattern::new_command("ls *");
        // With args — same as before.
        assert!(pat.matches("ls -la"));
        assert!(pat.matches("ls /tmp"));
        // Without args — NEW behavior post-F3.
        assert!(pat.matches("ls"));
        // Doesn't over-match a different command that happens to
        // start with `ls`.
        assert!(!pat.matches("lsof"));
        assert!(!pat.matches("less"));
    }

    /// F3 doesn't affect path-style patterns. `src/*` still
    /// matches single-segment files and doesn't span directories.
    /// (Note: `src/` itself matches because `*` accepts empty
    /// segments — pre-existing behavior, orthogonal to F3.)
    #[test]
    fn f3_does_not_relax_path_patterns() {
        let pat = Pattern::new("src/*");
        // With segment — matches.
        assert!(pat.matches("src/main.rs"));
        // Doesn't span directories (existing semantic).
        assert!(!pat.matches("src/agent/main.rs"));
        // Bare `src` (no trailing slash) — pre-F3 behavior:
        // doesn't match because pattern requires the `/`.
        assert!(!pat.matches("src"));
    }

    /// F3: bare `git *` doesn't accidentally swallow other commands.
    #[test]
    fn f3_anchored_to_command_head() {
        let pat = Pattern::new_command("git *");
        assert!(pat.matches("git"));
        assert!(pat.matches("git status"));
        assert!(pat.matches("git diff --name-only"));
        // Not anchored to a prefix; bare `git` matches but
        // `gitk` does not.
        assert!(!pat.matches("gitk"));
        assert!(!pat.matches("egit"));
    }

    // Regex metachars in pattern text must be escaped, not interpreted.
    #[test]
    fn special_chars_are_escaped() {
        let pat = Pattern::new_command("npm test (unit)");
        assert!(pat.matches("npm test (unit)"));
        // Without escaping, `(unit)` would be a regex group and not require
        // the literal parens.
        assert!(!pat.matches("npm test unit"));
    }
}

fn glob_to_regex(pattern: &str, path_style: bool) -> String {
    // F3 (dirge-efw): trailing ` *` becomes ` (?:.*)?$` — opencode's
    // `util/wildcard.ts:13-15` semantic. Lets a session-allowlist
    // pattern like `ls *` match BOTH `ls` (no args) and `ls -la`
    // (with args). Without this rewrite, `ls *` compiles to
    // `^ls .*$` which requires the trailing space, so the user
    // gets re-prompted for bare `ls`.
    //
    // Applies only to command-style patterns (path_style=false).
    // Path patterns like `src/*` legitimately require at least
    // one character after the slash; relaxing those would let
    // `src/` (the directory itself, no file) match a per-file
    // rule. Command tools use shell-style globbing where the
    // optional-trailing-arg semantic is the user expectation.
    if !path_style && pattern.ends_with(" *") && !pattern.ends_with("\\ *") {
        let head = &pattern[..pattern.len() - 2];
        let head_regex = glob_to_regex_inner(head, path_style);
        return format!("^{head_regex}(?: .*)?$");
    }
    format!("^{}$", glob_to_regex_inner(pattern, path_style))
}

/// Inner glob → regex without the leading `^` and trailing `$`
/// anchors. Separated so the F3 trailing-space-star rewrite can
/// wrap the head independently.
fn glob_to_regex_inner(pattern: &str, path_style: bool) -> String {
    let mut re = String::with_capacity(pattern.len() * 2);
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        re.push_str("(?:.*/)?");
                    } else {
                        re.push_str(".*");
                    }
                } else if path_style {
                    re.push_str("[^/]*");
                } else {
                    re.push_str(".*");
                }
            }
            '?' if path_style => re.push_str("[^/]"),
            '?' => re.push('.'),
            '.' => re.push_str("\\."),
            '\\' => re.push_str("\\\\"),
            '(' | ')' | '[' | ']' | '{' | '}' | '+' | '^' | '$' | '|' => {
                re.push('\\');
                re.push(c);
            }
            _ => re.push(c),
        }
    }
    re
}
