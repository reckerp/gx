//! Syntax highlighting for the diff view.
//!
//! syntect highlights whole-file content (its parse state is line-sequential,
//! so a hunk cannot be highlighted in isolation); the diff widget indexes the
//! per-line result back into its rows. Because syntect-tui pins an older
//! ratatui, we convert syntect's `(Style, &str)` segments to ratatui spans with
//! our own bridge ([`to_ratatui_style`]).
//!
//! A size guard keeps a large generated/vendored file (few changes, many lines)
//! from stalling the UI: past the guard, lines are returned as plain text.

use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

const DEFAULT_THEME: &str = "base16-ocean.dark";
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 5000;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

/// The ~250 bundled grammars load once, lazily, for the whole process.
fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    THEMES.get_or_init(ThemeSet::load_defaults)
}

/// A styled run of text within a single line.
pub type Segment = (Style, String);

/// Highlights file contents using a chosen theme. Cheap to construct (it only
/// borrows the process-global theme set).
pub struct Highlighter {
    theme: &'static Theme,
}

impl Highlighter {
    /// Build a highlighter for `theme_name`, falling back to the default theme
    /// and then to any available theme if the name is unknown.
    pub fn new(theme_name: &str) -> Self {
        let ts = theme_set();
        let theme = ts
            .themes
            .get(theme_name)
            .or_else(|| ts.themes.get(DEFAULT_THEME))
            .or_else(|| ts.themes.values().next())
            .expect("syntect ships at least one default theme");
        Highlighter { theme }
    }

    /// Highlight a whole file into line-indexed styled segments; callers index
    /// the result by `line_number - 1`. Returns one plain segment per line for
    /// files past the size guard.
    pub fn highlight_file(&self, path: &str, content: &str) -> Vec<Vec<Segment>> {
        if content.len() > MAX_HIGHLIGHT_BYTES || content.lines().count() > MAX_HIGHLIGHT_LINES {
            return plain_lines(content);
        }

        let ps = syntaxes();
        let syntax = syntax_for(ps, path, content);
        let mut highlighter = HighlightLines::new(syntax, self.theme);

        let mut out = Vec::new();
        for line in LinesWithEndings::from(content) {
            match highlighter.highlight_line(line, ps) {
                Ok(ranges) => out.push(to_segments(ranges)),
                // Degrade a problem line to plain text rather than dropping it.
                Err(_) => out.push(vec![(Style::default(), strip_eol(line))]),
            }
        }
        out
    }
}

fn plain_lines(content: &str) -> Vec<Vec<Segment>> {
    content
        .lines()
        .map(|l| vec![(Style::default(), l.to_string())])
        .collect()
}

fn to_segments(ranges: Vec<(SynStyle, &str)>) -> Vec<Segment> {
    let mut segs: Vec<Segment> = ranges
        .into_iter()
        .map(|(style, text)| (to_ratatui_style(style), text.to_string()))
        .collect();
    // The last segment carries the line's trailing newline; drop it.
    if let Some(last) = segs.last_mut() {
        while last.1.ends_with('\n') || last.1.ends_with('\r') {
            last.1.pop();
        }
        if last.1.is_empty() {
            segs.pop();
        }
    }
    segs
}

fn strip_eol(line: &str) -> String {
    line.trim_end_matches(['\n', '\r']).to_string()
}

/// Convert a syntect style to a ratatui style: RGB foreground plus font
/// modifiers. This is the bridge that lets us stay on ratatui 0.30 without the
/// `syntect-tui` crate (which pins ratatui 0.29).
pub fn to_ratatui_style(s: SynStyle) -> Style {
    let fg = s.foreground;
    let mut style = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if s.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

/// Pick a syntax by file extension, then by first line, falling back to plain text.
fn syntax_for<'a>(ps: &'a SyntaxSet, path: &str, content: &str) -> &'a SyntaxReference {
    if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str())
        && let Some(syntax) = ps.find_syntax_by_extension(ext)
    {
        return syntax;
    }
    if let Some(first) = content.lines().next()
        && let Some(syntax) = ps.find_syntax_by_first_line(first)
    {
        return syntax;
    }
    ps.find_syntax_plain_text()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use syntect::highlighting::Color as SynColor;

    #[test]
    fn bridge_maps_rgb_and_font_modifiers() {
        let syn = SynStyle {
            foreground: SynColor {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            },
            background: SynColor {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            font_style: FontStyle::BOLD | FontStyle::ITALIC,
        };
        let style = to_ratatui_style(syn);
        assert_eq!(style.fg, Some(Color::Rgb(10, 20, 30)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(!style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn highlights_rust_into_multiple_colors_and_keeps_line_count() {
        let h = Highlighter::new("base16-ocean.dark");
        let content = "fn main() {\n    let x = 1;\n}\n";
        let lines = h.highlight_file("main.rs", content);

        assert_eq!(lines.len(), 3, "one entry per source line");
        let colors: HashSet<_> = lines.iter().flatten().filter_map(|(s, _)| s.fg).collect();
        assert!(colors.len() > 1, "expected several syntax colors");
    }

    #[test]
    fn unknown_extension_does_not_panic() {
        let h = Highlighter::new("base16-ocean.dark");
        let lines = h.highlight_file("notes.unknownext", "hello world\n");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn unknown_theme_falls_back() {
        // Must not panic on a bogus theme name.
        let h = Highlighter::new("no-such-theme");
        assert_eq!(h.highlight_file("a.rs", "fn a(){}\n").len(), 1);
    }

    #[test]
    fn oversized_file_returns_plain_segments() {
        let h = Highlighter::new("base16-ocean.dark");
        let big = "x\n".repeat(MAX_HIGHLIGHT_LINES + 1);
        let lines = h.highlight_file("big.rs", &big);
        assert_eq!(lines.len(), MAX_HIGHLIGHT_LINES + 1);
        // Plain fallback: a single segment with no syntax foreground.
        assert_eq!(lines[0].len(), 1);
        assert!(lines[0][0].0.fg.is_none());
    }
}
