pub mod branch_picker;
pub mod clean_picker;
pub mod confirm;
pub mod file_picker;
pub mod log_viewer;
pub mod pr_picker;
pub mod setup_file_picker;
pub mod stash_picker;
pub mod status;
pub mod terminal;
pub mod workspace_picker;

use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::io::{Stderr, Stdout};

pub type Term = Terminal<CrosstermBackend<Stdout>>;
pub type TermStderr = Terminal<CrosstermBackend<Stderr>>;

pub fn render_help_bar(hints: &[(&str, &str)]) -> Paragraph<'static> {
    let mut spans = Vec::new();

    for (i, (key, action)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!(" {} ", key),
            Style::default().fg(Color::Yellow),
        ));
        spans.push(Span::styled(
            (*action).to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL).title(" Help "))
}

pub fn status_color(status: crate::git::status::FileStatus) -> Color {
    match status {
        crate::git::status::FileStatus::New => Color::Green,
        crate::git::status::FileStatus::Modified => Color::Yellow,
        crate::git::status::FileStatus::Deleted => Color::Red,
        crate::git::status::FileStatus::Renamed => Color::Cyan,
        crate::git::status::FileStatus::Typechange => Color::Magenta,
    }
}

pub fn status_char(status: crate::git::status::FileStatus) -> char {
    match status {
        crate::git::status::FileStatus::New => 'A',
        crate::git::status::FileStatus::Modified => 'M',
        crate::git::status::FileStatus::Deleted => 'D',
        crate::git::status::FileStatus::Renamed => 'R',
        crate::git::status::FileStatus::Typechange => 'T',
    }
}

/// Keep the selected row inside the visible window, returning the adjusted
/// scroll offset. The `visible_height == 0` guard matters: a viewport too short
/// to show any rows would otherwise underflow `visible_height - 1` (a debug-build
/// panic, a wrap in release). Shared by every scrolling picker.
pub(crate) fn adjust_scroll(selected: usize, scroll_offset: usize, visible_height: usize) -> usize {
    if visible_height == 0 {
        return scroll_offset;
    }

    if selected >= scroll_offset + visible_height {
        selected.saturating_sub(visible_height - 1)
    } else if selected < scroll_offset {
        selected
    } else {
        scroll_offset
    }
}

/// Truncate `s` to at most `max` characters (Unicode scalar values), appending
/// a single-character ellipsis ("…") when it had to be shortened. Shared by
/// every picker and status renderer so list rows truncate identically.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// A "shown range" label for list titles, e.g. `"11-12 of 12"` (or `"0 shown"`
/// when there is nothing to display).
pub(crate) fn visible_range(total: usize, scroll_offset: usize, visible_height: usize) -> String {
    if total == 0 || visible_height == 0 {
        return "0 shown".to_string();
    }

    let start = scroll_offset.min(total - 1) + 1;
    let end = (scroll_offset + visible_height).min(total);
    format!("{}-{} of {}", start, end, total)
}

/// A bordered single-line fuzzy-search input box titled `" Fuzzy Search "`.
pub(crate) fn render_search_bar(query: &str) -> Paragraph<'_> {
    Paragraph::new(query).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Fuzzy Search "),
    )
}

/// Fuzzy-filter `items` by `query`, returning the matches sorted best-first.
/// An empty query returns every item unchanged (and skips building the matcher,
/// since pickers re-filter on every frame). The `score` closure adapts each
/// item to the matcher — e.g. `|m, b| m.fuzzy_match(b, query)`.
pub(crate) fn fuzzy_filter<T: Clone>(
    items: &[T],
    query: &str,
    score: impl Fn(&SkimMatcherV2, &T) -> Option<i64>,
) -> Vec<T> {
    if query.is_empty() {
        return items.to_vec();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches: Vec<_> = items
        .iter()
        .filter_map(|item| score(&matcher, item).map(|s| (s, item)))
        .collect();
    matches.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    matches.into_iter().map(|(_, item)| item.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn test_visible_range() {
        assert_eq!(visible_range(0, 0, 5), "0 shown");
        assert_eq!(visible_range(12, 0, 5), "1-5 of 12");
        assert_eq!(visible_range(12, 10, 5), "11-12 of 12");
    }

    #[test]
    fn test_adjust_scroll_keeps_selection_visible() {
        assert_eq!(adjust_scroll(0, 0, 5), 0);
        assert_eq!(adjust_scroll(5, 0, 5), 1);
        assert_eq!(adjust_scroll(2, 5, 5), 2);
    }

    #[test]
    fn test_adjust_scroll_zero_height_is_noop() {
        // A zero-height viewport must not underflow; it just keeps the offset.
        assert_eq!(adjust_scroll(7, 3, 0), 3);
        assert_eq!(adjust_scroll(0, 0, 0), 0);
    }
}
