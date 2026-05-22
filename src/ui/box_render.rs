//! Shared box / chamber rendering for tool chambers, permission
//! alerts, panel sections, and any future framed UI block.
//!
//! Three glyph sets in the wild before this module:
//!   - Tool chambers: `╭─ NAME ─ "value" ─╮` … `│ content │` …
//!     `╰─────╯`. Built incrementally — TOP at ToolCall, BODY rows
//!     as ToolResult streams, BOTTOM at chamber close. Each row
//!     produced by a separate function call (`chamber_row`,
//!     `chamber_row_with_bg`, `chamber_bottom`).
//!   - Permission alert: `╭─ ⚠ ALERT · PERMISSION ──╮` … `│ row │`
//!     … `├──┤` … `│ row │` … `╰──╯`. Built all at once via a
//!     local `row` closure in the alert handler.
//!   - Panel sections: `╭─ HEADER ────╮` … `│ item │` …
//!     `╰────╯`. Built all at once via a `push_section` closure in
//!     `build_panel_lines`.
//!
//! The three implementations re-derived chamber math (frame_w,
//! inner, padding, truncation) separately, used inconsistent
//! width-counting (chamber_row was display-width-aware while
//! chamber_row_with_bg was char-count-based), and treated tab
//! expansion / ANSI escapes differently.
//!
//! This module unifies the row-painting primitives. Callers that
//! build a box all-at-once use `BoxBuilder`; callers that build
//! incrementally (tool chambers across multiple events) use the
//! raw `row`, `top`, `bottom`, `divider` helpers.

use compact_str::CompactString;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::wrap;

/// Visual style for a framed box. Currently only one shape ships
/// (rounded corners), but the enum gives a hook for future styles
/// (double-line, ASCII-fallback for legacy terminals).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxStyle {
    /// `╭─╮ │ │ ╰─╯` — the only style currently used. Rounded
    /// corners + light single-line borders.
    Rounded,
}

impl BoxStyle {
    pub fn top_left(self) -> char {
        match self {
            BoxStyle::Rounded => '╭',
        }
    }
    pub fn top_right(self) -> char {
        match self {
            BoxStyle::Rounded => '╮',
        }
    }
    pub fn bottom_left(self) -> char {
        match self {
            BoxStyle::Rounded => '╰',
        }
    }
    pub fn bottom_right(self) -> char {
        match self {
            BoxStyle::Rounded => '╯',
        }
    }
    pub fn horizontal(self) -> char {
        '─'
    }
    pub fn vertical(self) -> char {
        '│'
    }
    /// Left + right T-junctions used by divider rows inside a box.
    pub fn tee_left(self) -> char {
        '├'
    }
    pub fn tee_right(self) -> char {
        '┤'
    }
}

/// Build a `╭─ <title> ─<dashes>─╮` top border. `title` renders
/// flush against the left corner with one cell of dash padding on
/// each side; remaining width fills with horizontal dashes.
/// Width math is display-aware so wide title glyphs don't push the
/// right corner off.
pub fn top(style: BoxStyle, title: &str, total_w: usize) -> String {
    let title_w = UnicodeWidthStr::width(title);
    // Layout: `╭─ {title} ─{fill}─╮`
    // Cells fixed regardless of title/fill:
    //   ╭ (1) + ─ (1) + ' ' (1) + title + ' ' (1) + ─ (1)
    //   + fill + ─ (1) + ╮ (1) = title_w + fill + 7
    // So fill = total_w − title_w − 7. Saturating-sub keeps the
    // call safe when total_w is tiny (degenerate terminals).
    const OVERHEAD: usize = 7;
    let fill = total_w.saturating_sub(OVERHEAD).saturating_sub(title_w);
    format!(
        "{}─ {} ─{}─{}",
        style.top_left(),
        title,
        style.horizontal().to_string().repeat(fill),
        style.top_right(),
    )
}

/// Build a `╰─────╯` bottom border sized to `total_w`.
pub fn bottom(style: BoxStyle, total_w: usize) -> String {
    let inner = total_w.saturating_sub(2); // corners
    format!(
        "{}{}{}",
        style.bottom_left(),
        style.horizontal().to_string().repeat(inner),
        style.bottom_right(),
    )
}

/// Build a `├─────┤` divider row sized to `total_w`. Used inside
/// boxes that have multiple sections (e.g. the permission alert
/// separates the question and action rows with a divider).
pub fn divider(style: BoxStyle, total_w: usize) -> String {
    let inner = total_w.saturating_sub(2);
    format!(
        "{}{}{}",
        style.tee_left(),
        style.horizontal().to_string().repeat(inner),
        style.tee_right(),
    )
}

/// Build a `│ content {pad} │` content row sized so the right
/// border lands at column `total_w`. `total_w` is the EXTERNAL
/// width (border-to-border). Content width is `total_w - 4` —
/// two cells for each border + space.
///
/// Long content is truncated with `…`. Tabs are expanded to
/// `tab_stop` spaces beforehand so chamber rows stay aligned
/// regardless of where the tab fell. Width is display-aware so
/// wide glyphs don't drift the right border.
///
/// Callers that need their content soft-wrapped across multiple
/// rows should use `wrap::soft_wrap` themselves to chunk the input
/// and call `row` once per chunk.
pub fn row(style: BoxStyle, content: &str, total_w: usize) -> String {
    let inner = total_w.saturating_sub(4);
    let expanded = expand_tabs(content, 4);
    let total_visible = UnicodeWidthStr::width(expanded.as_str());

    let (trimmed, trimmed_w): (String, usize) = if total_visible <= inner {
        (expanded.clone(), total_visible)
    } else if inner == 0 {
        (String::new(), 0)
    } else {
        // Reserve 1 cell for `…`; pull chars from the start until
        // we'd overflow the remaining budget.
        let budget = inner.saturating_sub(1);
        let mut out = String::with_capacity(expanded.len());
        let mut used = 0;
        for ch in expanded.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > budget {
                break;
            }
            out.push(ch);
            used += w;
        }
        out.push('…');
        (out, used + 1)
    };
    let pad = inner.saturating_sub(trimmed_w);
    format!(
        "{} {}{} {}",
        style.vertical(),
        trimmed,
        " ".repeat(pad),
        style.vertical(),
    )
}

/// Same as `row` but wraps content with a 256-color background
/// inside the borders. Used by diff `+`/`-` rows where the BG
/// signals add/remove. Border glyphs sit OUTSIDE the bg span so
/// they keep the chamber color.
pub fn row_with_bg(style: BoxStyle, content: &str, total_w: usize, bg_idx: u8) -> String {
    let inner = total_w.saturating_sub(4);
    let expanded = expand_tabs(content, 4);
    let chars: Vec<char> = expanded.chars().collect();
    let trimmed: String = if chars.len() <= inner {
        chars.iter().collect()
    } else if inner == 0 {
        String::new()
    } else {
        let mut out: String = chars[..inner.saturating_sub(1)].iter().collect();
        out.push('…');
        out
    };
    let pad = inner.saturating_sub(trimmed.chars().count());
    format!(
        "{} \x1b[48;5;{}m{}{}\x1b[49m {}",
        style.vertical(),
        bg_idx,
        trimmed,
        " ".repeat(pad),
        style.vertical(),
    )
}

/// Expand `\t` to spaces honouring a fixed tab stop. Walks the
/// string tracking column position so a tab N cells before the
/// next stop expands to exactly `stop - (col % stop)` spaces.
/// Display-width-aware: a wide glyph advances col by 2.
pub fn expand_tabs(s: &str, tab_stop: usize) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut col = 0usize;
    for ch in s.chars() {
        if ch == '\t' {
            let pad = tab_stop - (col % tab_stop);
            for _ in 0..pad {
                out.push(' ');
            }
            col += pad;
        } else {
            out.push(ch);
            col += ch.width().unwrap_or(0);
        }
    }
    out
}

// === Builder ===
//
// Convenience for callers that build a complete box in one go
// (alerts, panel sections, notifications). Construct, push rows
// (optionally with dividers), finalize to a Vec<String> the
// caller paints in order.

pub struct BoxBuilder {
    style: BoxStyle,
    width: usize,
    title: String,
    rows: Vec<RowKind>,
}

enum RowKind {
    Text(CompactString),
    Divider,
}

impl BoxBuilder {
    /// Start a new box. `width` is the external width
    /// (border-to-border). `title` renders in the top border.
    pub fn new(style: BoxStyle, title: impl Into<String>, width: usize) -> Self {
        Self {
            style,
            width: width.max(8),
            title: title.into(),
            rows: Vec::new(),
        }
    }

    /// Push a content row. If the row exceeds the inner width it
    /// soft-wraps via `wrap::soft_wrap` — every wrapped chunk
    /// becomes its own framed row. Empty input emits a single
    /// blank row (useful for vertical padding).
    pub fn row(mut self, content: impl AsRef<str>) -> Self {
        let inner = self.width.saturating_sub(4);
        let s = content.as_ref();
        if s.is_empty() {
            self.rows.push(RowKind::Text(CompactString::new("")));
            return self;
        }
        for chunk in wrap::soft_wrap(s, inner, "") {
            self.rows.push(RowKind::Text(CompactString::from(chunk)));
        }
        self
    }

    /// Push a horizontal divider (`├──┤`) between rows.
    pub fn divider(mut self) -> Self {
        self.rows.push(RowKind::Divider);
        self
    }

    /// Finalise to a Vec<String> ready to paint top-to-bottom.
    pub fn build(self) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(self.rows.len() + 2);
        out.push(top(self.style, &self.title, self.width));
        for rk in self.rows {
            match rk {
                RowKind::Text(s) => out.push(row(self.style, &s, self.width)),
                RowKind::Divider => out.push(divider(self.style, self.width)),
            }
        }
        out.push(bottom(self.style, self.width));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Top, bottom, and divider all hit the requested external width.
    #[test]
    fn frame_helpers_match_total_width() {
        for w in [12, 60, 120usize] {
            let t = top(BoxStyle::Rounded, "TEST", w);
            let b = bottom(BoxStyle::Rounded, w);
            let d = divider(BoxStyle::Rounded, w);
            assert_eq!(UnicodeWidthStr::width(t.as_str()), w, "top width@{w}");
            assert_eq!(UnicodeWidthStr::width(b.as_str()), w, "bottom width@{w}");
            assert_eq!(UnicodeWidthStr::width(d.as_str()), w, "divider width@{w}");
        }
    }

    /// Content rows match total_w independent of input content
    /// (short, exact, overflowing).
    #[test]
    fn row_width_invariant() {
        let w = 30;
        for input in &["short", "exactlyfittingrow", &"x".repeat(100)] {
            let r = row(BoxStyle::Rounded, input, w);
            assert_eq!(UnicodeWidthStr::width(r.as_str()), w, "row({input:?})@{w}");
        }
    }

    /// Tabs expand BEFORE width measurement so the right border
    /// doesn't drift.
    #[test]
    fn row_handles_tabs() {
        let w = 40;
        let r = row(BoxStyle::Rounded, "a\tb", w);
        assert_eq!(UnicodeWidthStr::width(r.as_str()), w);
        assert!(!r.contains('\t'));
    }

    /// CJK glyphs count as width-2.
    #[test]
    fn row_handles_cjk() {
        let w = 30;
        let r = row(BoxStyle::Rounded, "中文测试", w);
        assert_eq!(UnicodeWidthStr::width(r.as_str()), w);
    }

    /// Builder produces a sequence: top, then rows in order, then
    /// bottom. Every output line hits the requested width.
    #[test]
    fn builder_produces_well_formed_box() {
        let out = BoxBuilder::new(BoxStyle::Rounded, "TITLE", 40)
            .row("first line")
            .row("second line")
            .divider()
            .row("after divider")
            .build();
        assert!(out.len() >= 5);
        // First is top border, last is bottom border.
        assert!(out[0].starts_with('╭'));
        assert!(out.last().unwrap().starts_with('╰'));
        // All rows the same external width.
        for line in &out {
            assert_eq!(UnicodeWidthStr::width(line.as_str()), 40);
        }
    }

    /// A row longer than the inner width wraps across multiple
    /// rows rather than truncating with `…`.
    #[test]
    fn builder_soft_wraps_long_rows() {
        let long = "the quick brown fox jumps over the lazy dog repeatedly";
        let out = BoxBuilder::new(BoxStyle::Rounded, "T", 30)
            .row(long)
            .build();
        // Top + N wrapped rows + bottom; N ≥ 2 for this length.
        let content_rows = out.len() - 2;
        assert!(content_rows >= 2, "expected wrap, got {content_rows} rows");
        // None of the rows truncated with `…`.
        for line in &out[1..out.len() - 1] {
            assert!(!line.contains('…'), "row truncated unexpectedly: {line}");
        }
    }

    /// `expand_tabs` honours mixed content + tab stop alignment.
    #[test]
    fn expand_tabs_aligned_to_stop() {
        assert_eq!(expand_tabs("a\tb", 4), "a   b");
        assert_eq!(expand_tabs("ab\tc", 4), "ab  c");
        assert_eq!(expand_tabs("abc\td", 4), "abc d");
        assert_eq!(expand_tabs("abcd\te", 4), "abcd    e");
    }
}
