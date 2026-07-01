//! The side-by-side (and unified-fallback) diff widget.
//!
//! A [`RenderedFile`] bundles a file's [`FileDiff`] with its whole-file syntax
//! highlighting. Rendering layers three signals per line — syntax color (from
//! the highlighter), a diff background (add/remove), and word-level emphasis
//! (brighter background on the changed byte ranges) — the way `git-delta` does.
//!
//! Side-by-side pairs each run of removed lines with the following run of added
//! lines into left/right columns; unified shows the flat row sequence in one
//! column. Both are derived purely from the [`FileDiff`], so the pairing logic
//! is unit-tested without a terminal.

use crate::git::GitError;
use crate::git::review::diff::{ChangedFile, FileDiff, Row, RowKind};
use crate::git::review::range::Endpoint;
use crate::git::review::state::{Marks, Side};
use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use super::Appearance;
use super::color::{self, ColorDepth};
use super::highlight::{Highlighter, Segment};

/// Colors for the diff view, chosen for the terminal's light or dark
/// background. Context lines use `Color::Reset` (the terminal's own background)
/// so they read correctly either way.
#[derive(Clone, Copy)]
pub struct Palette {
    pub add_bg: Color,
    pub add_emph_bg: Color,
    pub del_bg: Color,
    pub del_emph_bg: Color,
    pub cursor_bg: Color,
    pub select_bg: Color,
    pub empty_bg: Color,
    pub gutter_fg: Color,
    pub header_fg: Color,
}

impl Palette {
    pub fn for_appearance(appearance: Appearance, depth: ColorDepth) -> Self {
        let base = match appearance {
            Appearance::Dark => Palette::dark(),
            Appearance::Light => Palette::light(),
        };
        base.adapted(depth)
    }

    /// Downsample every RGB field to the terminal's color depth so the diff
    /// backgrounds survive on 256-color terminals (named colors pass through).
    fn adapted(self, depth: ColorDepth) -> Self {
        let a = |c: Color| color::adapt(c, depth);
        Palette {
            add_bg: a(self.add_bg),
            add_emph_bg: a(self.add_emph_bg),
            del_bg: a(self.del_bg),
            del_emph_bg: a(self.del_emph_bg),
            cursor_bg: a(self.cursor_bg),
            select_bg: a(self.select_bg),
            empty_bg: a(self.empty_bg),
            gutter_fg: a(self.gutter_fg),
            header_fg: a(self.header_fg),
        }
    }

    fn dark() -> Self {
        Palette {
            add_bg: Color::Rgb(22, 43, 28),
            add_emph_bg: Color::Rgb(36, 84, 46),
            del_bg: Color::Rgb(58, 28, 32),
            del_emph_bg: Color::Rgb(102, 44, 50),
            cursor_bg: Color::Rgb(50, 50, 70),
            select_bg: Color::Rgb(50, 50, 70),
            empty_bg: Color::Rgb(30, 30, 34),
            gutter_fg: Color::Rgb(120, 120, 135),
            header_fg: Color::Cyan,
        }
    }

    fn light() -> Self {
        Palette {
            add_bg: Color::Rgb(216, 245, 220),
            add_emph_bg: Color::Rgb(150, 222, 162),
            del_bg: Color::Rgb(250, 215, 215),
            del_emph_bg: Color::Rgb(245, 168, 174),
            cursor_bg: Color::Rgb(206, 215, 242),
            select_bg: Color::Rgb(206, 215, 242),
            empty_bg: Color::Rgb(236, 236, 238),
            gutter_fg: Color::Rgb(120, 120, 130),
            header_fg: Color::Rgb(0, 92, 130),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    SideBySide,
    Unified,
}

/// A file's diff plus its whole-file syntax highlighting (line-indexed).
pub struct RenderedFile {
    pub diff: FileDiff,
    old_hl: Vec<Vec<Segment>>,
    new_hl: Vec<Vec<Segment>>,
}

/// Build the diff and highlighting for one changed file. Binary / oversized
/// files skip highlighting (they render a placeholder).
pub fn render_file(
    file: &ChangedFile,
    to: Endpoint,
    highlighter: &Highlighter,
) -> Result<RenderedFile, GitError> {
    let diff = file.build(to)?;
    let (old_hl, new_hl) = if diff.is_binary || diff.too_large {
        (Vec::new(), Vec::new())
    } else {
        let (old, new) = file.load_contents(to)?;
        (
            highlighter.highlight_file(&file.path, &old),
            highlighter.highlight_file(&file.path, &new),
        )
    };
    Ok(RenderedFile {
        diff,
        old_hl,
        new_hl,
    })
}

// --- Visual models (pure, derived from the diff) ---------------------------

enum SideLine<'a> {
    Header(&'a str),
    Pair {
        left: Option<&'a Row>,
        right: Option<&'a Row>,
    },
}

enum UniLine<'a> {
    Header(&'a str),
    Row(&'a Row),
}

fn side_lines(diff: &FileDiff) -> Vec<SideLine<'_>> {
    let mut out = Vec::new();
    for hunk in &diff.hunks {
        out.push(SideLine::Header(&hunk.header));
        let mut removed: Vec<&Row> = Vec::new();
        let mut added: Vec<&Row> = Vec::new();
        for row in &hunk.rows {
            match row.kind {
                RowKind::Context => {
                    flush_pairs(&mut removed, &mut added, &mut out);
                    out.push(SideLine::Pair {
                        left: Some(row),
                        right: Some(row),
                    });
                }
                RowKind::Removed => removed.push(row),
                RowKind::Added => added.push(row),
            }
        }
        flush_pairs(&mut removed, &mut added, &mut out);
    }
    out
}

fn flush_pairs<'a>(
    removed: &mut Vec<&'a Row>,
    added: &mut Vec<&'a Row>,
    out: &mut Vec<SideLine<'a>>,
) {
    let n = removed.len().max(added.len());
    for i in 0..n {
        out.push(SideLine::Pair {
            left: removed.get(i).copied(),
            right: added.get(i).copied(),
        });
    }
    removed.clear();
    added.clear();
}

fn uni_lines(diff: &FileDiff) -> Vec<UniLine<'_>> {
    let mut out = Vec::new();
    for hunk in &diff.hunks {
        out.push(UniLine::Header(&hunk.header));
        for row in &hunk.rows {
            out.push(UniLine::Row(row));
        }
    }
    out
}

/// Number of navigable visual lines for the current view.
pub fn line_count(rf: &RenderedFile, view: ViewMode) -> usize {
    match view {
        ViewMode::SideBySide => side_lines(&rf.diff).len(),
        ViewMode::Unified => uni_lines(&rf.diff).len(),
    }
}

/// Visual-line indices of hunk headers, for next/prev-hunk navigation.
pub fn hunk_header_indices(rf: &RenderedFile, view: ViewMode) -> Vec<usize> {
    match view {
        ViewMode::SideBySide => side_lines(&rf.diff)
            .iter()
            .enumerate()
            .filter_map(|(i, l)| matches!(l, SideLine::Header(_)).then_some(i))
            .collect(),
        ViewMode::Unified => uni_lines(&rf.diff)
            .iter()
            .enumerate()
            .filter_map(|(i, l)| matches!(l, UniLine::Header(_)).then_some(i))
            .collect(),
    }
}

// --- Anchoring (cursor -> comment target) -----------------------------------

/// Where a comment would attach for a given cursor position.
pub struct Anchor {
    pub side: Side,
    pub line: usize,
    pub text: String,
}

/// Resolve the cursor's visual line to a comment anchor, preferring the new
/// side (the post-change line an agent acts on). Returns `None` for
/// non-anchorable rows (hunk headers, side-by-side gap rows).
pub fn anchor_at(rf: &RenderedFile, view: ViewMode, cursor: usize) -> Option<Anchor> {
    match view {
        ViewMode::SideBySide => match side_lines(&rf.diff).get(cursor)? {
            SideLine::Header(_) => None,
            SideLine::Pair { left, right } => pair_anchor(*left, *right),
        },
        ViewMode::Unified => match uni_lines(&rf.diff).get(cursor)? {
            UniLine::Header(_) => None,
            UniLine::Row(row) => row_anchor(row),
        },
    }
}

fn pair_anchor(left: Option<&Row>, right: Option<&Row>) -> Option<Anchor> {
    if let Some(r) = right
        && let Some(line) = r.new_no
    {
        return Some(Anchor {
            side: Side::New,
            line,
            text: r.text.clone(),
        });
    }
    if let Some(l) = left
        && let Some(line) = l.old_no
    {
        return Some(Anchor {
            side: Side::Old,
            line,
            text: l.text.clone(),
        });
    }
    None
}

fn row_anchor(row: &Row) -> Option<Anchor> {
    if let Some(line) = row.new_no {
        Some(Anchor {
            side: Side::New,
            line,
            text: row.text.clone(),
        })
    } else {
        row.old_no.map(|line| Anchor {
            side: Side::Old,
            line,
            text: row.text.clone(),
        })
    }
}

/// Resolve a visual-line range `[lo, hi]` to a single multi-line anchor: the
/// side of the first anchorable row, spanning min..max line on that side.
pub fn anchor_span(
    rf: &RenderedFile,
    view: ViewMode,
    lo: usize,
    hi: usize,
) -> Option<(Side, usize, usize, String)> {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let mut side: Option<Side> = None;
    let mut min = usize::MAX;
    let mut max = 0usize;
    let mut text = String::new();

    for i in lo..=hi {
        if let Some(a) = anchor_at(rf, view, i) {
            let s = *side.get_or_insert(a.side);
            if a.side == s {
                if a.line < min {
                    min = a.line;
                    text = a.text.clone();
                }
                max = max.max(a.line);
            }
        }
    }

    side.map(|s| (s, min, max, text))
}

// --- Rendering --------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    rf: &RenderedFile,
    marks: &Marks,
    view: ViewMode,
    cursor: usize,
    v_scroll: usize,
    h_scroll: usize,
    focused: bool,
    palette: Palette,
    tab_width: usize,
) {
    let path_label = match &rf.diff.old_path {
        Some(old) => format!("{old} → {}", rf.diff.path),
        None => rf.diff.path.clone(),
    };
    let title = format!(
        " {} {} {} ",
        crate::ui::status_char(rf.diff.status),
        path_label,
        match view {
            ViewMode::SideBySide => "[split]",
            ViewMode::Unified => "[unified]",
        }
    );
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if rf.diff.is_binary {
        return placeholder(f, inner, "Binary file — no text diff");
    }
    if rf.diff.too_large {
        return placeholder(f, inner, "File too large to diff");
    }
    if rf.diff.hunks.is_empty() {
        return placeholder(f, inner, "No changes in this file");
    }

    let height = inner.height as usize;
    let width = inner.width as usize;
    let gw = gutter_width(&rf.diff);

    let lines: Vec<Line> = match view {
        ViewMode::SideBySide => {
            let model = side_lines(&rf.diff);
            visible_range(model.len(), v_scroll, height)
                .map(|i| {
                    side_line_to_line(
                        &model[i],
                        rf,
                        marks,
                        gw,
                        width,
                        h_scroll,
                        focused && i == cursor,
                        palette,
                        tab_width,
                    )
                })
                .collect()
        }
        ViewMode::Unified => {
            let model = uni_lines(&rf.diff);
            visible_range(model.len(), v_scroll, height)
                .map(|i| {
                    uni_line_to_line(
                        &model[i],
                        rf,
                        marks,
                        gw,
                        width,
                        h_scroll,
                        focused && i == cursor,
                        palette,
                        tab_width,
                    )
                })
                .collect()
        }
    };

    f.render_widget(Paragraph::new(lines), inner);
}

fn placeholder(f: &mut Frame, area: Rect, msg: &str) {
    let p = Paragraph::new(Line::from(Span::styled(
        msg.to_string(),
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(p, area);
}

fn visible_range(total: usize, v_scroll: usize, height: usize) -> std::ops::Range<usize> {
    let start = v_scroll.min(total);
    let end = (start + height).min(total);
    start..end
}

fn gutter_width(diff: &FileDiff) -> usize {
    let mut max = 0usize;
    for hunk in &diff.hunks {
        for row in &hunk.rows {
            max = max.max(row.old_no.unwrap_or(0)).max(row.new_no.unwrap_or(0));
        }
    }
    max.to_string().len().max(3)
}

#[allow(clippy::too_many_arguments)]
fn side_line_to_line<'a>(
    line: &SideLine<'a>,
    rf: &'a RenderedFile,
    marks: &Marks,
    gw: usize,
    width: usize,
    h_scroll: usize,
    cursor: bool,
    pal: Palette,
    tab_width: usize,
) -> Line<'a> {
    // First column is a 1-cell comment marker; the rest is the body.
    let body_width = width.saturating_sub(1);
    match line {
        SideLine::Header(h) => {
            let mut spans = vec![marker_span(false, cursor, pal)];
            spans.extend(header_spans(h, body_width, cursor, pal));
            Line::from(spans)
        }
        SideLine::Pair { left, right } => {
            let sep_cols = 3usize;
            let half = body_width.saturating_sub(sep_cols) / 2;
            let text_w = half.saturating_sub(gw + 1);

            let mut spans = vec![marker_span(pair_marked(*left, *right, marks), cursor, pal)];
            spans.extend(cell(
                *left,
                segs(&rf.old_hl, left.and_then(|r| r.old_no)),
                Col::Old,
                gw,
                text_w,
                h_scroll,
                cursor,
                pal,
                tab_width,
            ));
            spans.push(Span::styled(
                " │ ",
                Style::default().fg(Color::DarkGray).bg(sep_bg(cursor, pal)),
            ));
            spans.extend(cell(
                *right,
                segs(&rf.new_hl, right.and_then(|r| r.new_no)),
                Col::New,
                gw,
                text_w,
                h_scroll,
                cursor,
                pal,
                tab_width,
            ));
            Line::from(spans)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn uni_line_to_line<'a>(
    line: &UniLine<'a>,
    rf: &'a RenderedFile,
    marks: &Marks,
    gw: usize,
    width: usize,
    h_scroll: usize,
    cursor: bool,
    pal: Palette,
    tab_width: usize,
) -> Line<'a> {
    let body_width = width.saturating_sub(1);
    match line {
        UniLine::Header(h) => {
            let mut spans = vec![marker_span(false, cursor, pal)];
            spans.extend(header_spans(h, body_width, cursor, pal));
            Line::from(spans)
        }
        UniLine::Row(row) => {
            let sign = match row.kind {
                RowKind::Added => "+",
                RowKind::Removed => "-",
                RowKind::Context => " ",
            };
            let bg = row_bg(row.kind, cursor, pal);
            let text_w = body_width.saturating_sub(gw * 2 + 4);
            let segments = match row.kind {
                RowKind::Removed => segs(&rf.old_hl, row.old_no),
                _ => segs(&rf.new_hl, row.new_no),
            };

            let mut spans = vec![
                marker_span(uni_marked(row, marks), cursor, pal),
                num_span(row.old_no, gw, bg, pal),
                Span::styled(" ", Style::default().bg(bg)),
                num_span(row.new_no, gw, bg, pal),
                Span::styled(format!(" {sign} "), marker_style(row.kind, cursor, pal)),
            ];
            spans.extend(text_spans(
                &row.text,
                segments,
                &row.emphasis,
                row.kind,
                cursor,
                h_scroll,
                text_w,
                pal,
                tab_width,
            ));
            Line::from(spans)
        }
    }
}

enum Col {
    Old,
    New,
}

/// One half of a side-by-side line: gutter + the line's styled text (or blank
/// when this side has no corresponding line).
#[allow(clippy::too_many_arguments)]
fn cell<'a>(
    row: Option<&'a Row>,
    segments: &'a [Segment],
    col: Col,
    gw: usize,
    text_w: usize,
    h_scroll: usize,
    cursor: bool,
    pal: Palette,
    tab_width: usize,
) -> Vec<Span<'a>> {
    let Some(row) = row else {
        // No line on this side: blank gutter + filler at the empty-side bg.
        let bg = if cursor { pal.cursor_bg } else { pal.empty_bg };
        return vec![
            Span::styled(" ".repeat(gw + 1), Style::default().bg(bg)),
            Span::styled(" ".repeat(text_w), Style::default().bg(bg)),
        ];
    };
    let num = match col {
        Col::Old => row.old_no,
        Col::New => row.new_no,
    };
    let bg = row_bg(row.kind, cursor, pal);
    let mut spans = vec![
        num_span(num, gw, bg, pal),
        Span::styled(" ", Style::default().bg(bg)),
    ];
    spans.extend(text_spans(
        &row.text,
        segments,
        &row.emphasis,
        row.kind,
        cursor,
        h_scroll,
        text_w,
        pal,
        tab_width,
    ));
    spans
}

fn header_spans<'a>(header: &str, width: usize, cursor: bool, pal: Palette) -> Vec<Span<'a>> {
    let bg = if cursor { pal.cursor_bg } else { Color::Reset };
    let mut text = header.to_string();
    let count = text.chars().count();
    if count < width {
        text.push_str(&" ".repeat(width - count));
    }
    vec![Span::styled(
        text,
        Style::default()
            .fg(pal.header_fg)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    )]
}

fn marker_span<'a>(marked: bool, cursor: bool, pal: Palette) -> Span<'a> {
    let bg = if cursor { pal.cursor_bg } else { Color::Reset };
    let (ch, fg) = if marked {
        ("●", Color::Magenta)
    } else {
        (" ", Color::Reset)
    };
    // `ch` is a &'static str literal — no per-line allocation.
    Span::styled(ch, Style::default().fg(fg).bg(bg))
}

fn pair_marked(left: Option<&Row>, right: Option<&Row>, marks: &Marks) -> bool {
    let l = left
        .and_then(|r| r.old_no)
        .is_some_and(|n| marks.old.contains(&n));
    let r = right
        .and_then(|r| r.new_no)
        .is_some_and(|n| marks.new.contains(&n));
    l || r
}

fn uni_marked(row: &Row, marks: &Marks) -> bool {
    row.new_no.is_some_and(|n| marks.new.contains(&n))
        || row.old_no.is_some_and(|n| marks.old.contains(&n))
}

fn num_span<'a>(num: Option<usize>, gw: usize, bg: Color, pal: Palette) -> Span<'a> {
    let s = match num {
        Some(n) => format!("{n:>gw$}"),
        None => " ".repeat(gw),
    };
    Span::styled(s, Style::default().fg(pal.gutter_fg).bg(bg))
}

/// Build the styled, horizontally-scrolled, width-padded text spans for a line,
/// layering syntax color, diff background, and word-level emphasis. `text` is
/// the raw line, used as a fallback when no syntax segments are available.
///
/// Tabs are expanded to `tab_width`-aligned spaces so indentation lines up — a
/// raw `\t` in a ratatui span otherwise renders as a single cell and collapses
/// indentation. Emphasis is keyed off the byte offset in `text`; the syntax
/// segments concatenate to the same bytes, so tracking bytes as we walk either
/// keeps both aligned.
#[allow(clippy::too_many_arguments)]
fn text_spans<'a>(
    text: &str,
    segments: &[Segment],
    emphasis: &[(usize, usize)],
    kind: RowKind,
    cursor: bool,
    h_scroll: usize,
    width: usize,
    pal: Palette,
    tab_width: usize,
) -> Vec<Span<'a>> {
    let base_bg = row_bg(kind, cursor, pal);
    let emph_bg = emph_bg(kind, cursor, pal);
    let tab_width = tab_width.max(1);

    // Expand to per-cell (char, style), tracking byte offset for emphasis and
    // visual column for tab stops.
    let mut chars: Vec<(char, Style)> = Vec::new();
    let mut byte = 0usize;
    let mut push_char = |ch: char, fg: Option<Color>| {
        let emphasized = in_ranges(byte, emphasis);
        let bg = if emphasized { emph_bg } else { base_bg };
        byte += ch.len_utf8();
        if ch == '\t' {
            // Advance to the next tab stop, filling with the line's background.
            let fill = tab_width - (chars.len() % tab_width);
            for _ in 0..fill {
                chars.push((' ', Style::default().bg(bg)));
            }
            return;
        }
        let mut style = Style::default().bg(bg);
        if let Some(c) = fg {
            style = style.fg(c);
        }
        chars.push((ch, style));
    };

    if segments.is_empty() {
        // No syntax segments (blank line, or highlighting unavailable): render
        // the raw text so content is never dropped.
        for ch in text.chars() {
            push_char(ch, None);
        }
    } else {
        for (seg_style, seg_text) in segments {
            for ch in seg_text.chars() {
                push_char(ch, seg_style.fg);
            }
        }
    }

    // Horizontal scroll + width clamp, then pad to full width so the bg fills.
    let mut visible: Vec<(char, Style)> = chars.into_iter().skip(h_scroll).take(width).collect();
    let pad = width.saturating_sub(visible.len());
    for _ in 0..pad {
        visible.push((' ', Style::default().bg(base_bg)));
    }
    coalesce(visible)
}

fn coalesce<'a>(chars: Vec<(char, Style)>) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut cur = String::new();
    let mut cur_style: Option<Style> = None;
    for (ch, style) in chars {
        if cur_style != Some(style) {
            if let Some(s) = cur_style
                && !cur.is_empty()
            {
                spans.push(Span::styled(std::mem::take(&mut cur), s));
            }
            cur_style = Some(style);
        }
        cur.push(ch);
    }
    if let Some(s) = cur_style
        && !cur.is_empty()
    {
        spans.push(Span::styled(cur, s));
    }
    spans
}

fn in_ranges(byte: usize, ranges: &[(usize, usize)]) -> bool {
    ranges.iter().any(|&(s, e)| byte >= s && byte < e)
}

fn segs(hl: &[Vec<Segment>], line_no: Option<usize>) -> &[Segment] {
    line_no
        .and_then(|n| hl.get(n.saturating_sub(1)))
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

fn row_bg(kind: RowKind, cursor: bool, pal: Palette) -> Color {
    if cursor {
        return pal.cursor_bg;
    }
    match kind {
        RowKind::Added => pal.add_bg,
        RowKind::Removed => pal.del_bg,
        RowKind::Context => Color::Reset,
    }
}

fn emph_bg(kind: RowKind, cursor: bool, pal: Palette) -> Color {
    if cursor {
        return pal.cursor_bg;
    }
    match kind {
        RowKind::Added => pal.add_emph_bg,
        RowKind::Removed => pal.del_emph_bg,
        RowKind::Context => Color::Reset,
    }
}

fn sep_bg(cursor: bool, pal: Palette) -> Color {
    if cursor { pal.cursor_bg } else { Color::Reset }
}

fn marker_style(kind: RowKind, cursor: bool, pal: Palette) -> Style {
    let fg = match kind {
        RowKind::Added => Color::Green,
        RowKind::Removed => Color::Red,
        RowKind::Context => Color::DarkGray,
    };
    Style::default().fg(fg).bg(row_bg(kind, cursor, pal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::review::diff::Hunk;
    use crate::git::status::FileStatus;

    fn row(kind: RowKind, old: Option<usize>, new: Option<usize>, text: &str) -> Row {
        Row {
            kind,
            old_no: old,
            new_no: new,
            text: text.to_string(),
            emphasis: Vec::new(),
        }
    }

    fn diff_with(rows: Vec<Row>) -> FileDiff {
        FileDiff {
            path: "x.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            too_large: false,
            hunks: vec![Hunk {
                header: "@@ -1,3 +1,3 @@".into(),
                rows,
            }],
        }
    }

    #[test]
    fn side_pairs_removed_with_added() {
        // context, removed b, added B, context  ->  header + 3 pairs
        let diff = diff_with(vec![
            row(RowKind::Context, Some(1), Some(1), "a"),
            row(RowKind::Removed, Some(2), None, "b"),
            row(RowKind::Added, None, Some(2), "B"),
            row(RowKind::Context, Some(3), Some(3), "c"),
        ]);
        let lines = side_lines(&diff);
        // header + a + (b|B) + c
        assert_eq!(lines.len(), 4);
        match &lines[2] {
            SideLine::Pair { left, right } => {
                assert_eq!(left.unwrap().text, "b");
                assert_eq!(right.unwrap().text, "B");
            }
            _ => panic!("expected a paired removed/added line"),
        }
    }

    #[test]
    fn side_pairs_uneven_runs_with_blanks() {
        // 2 removed, 1 added -> 2 pair lines; second has no right side.
        let diff = diff_with(vec![
            row(RowKind::Removed, Some(1), None, "x"),
            row(RowKind::Removed, Some(2), None, "y"),
            row(RowKind::Added, None, Some(1), "Z"),
        ]);
        let lines = side_lines(&diff);
        // header + 2 pair lines
        assert_eq!(lines.len(), 3);
        match &lines[2] {
            SideLine::Pair { left, right } => {
                assert_eq!(left.unwrap().text, "y");
                assert!(right.is_none());
            }
            _ => panic!("expected pair"),
        }
    }

    #[test]
    fn unified_keeps_flat_order_with_header() {
        let diff = diff_with(vec![
            row(RowKind::Context, Some(1), Some(1), "a"),
            row(RowKind::Added, None, Some(2), "b"),
        ]);
        let lines = uni_lines(&diff);
        assert_eq!(lines.len(), 3); // header + 2 rows
        assert!(matches!(lines[0], UniLine::Header(_)));
    }

    #[test]
    fn hunk_indices_point_at_headers() {
        let rf = RenderedFile {
            diff: diff_with(vec![row(RowKind::Context, Some(1), Some(1), "a")]),
            old_hl: vec![],
            new_hl: vec![],
        };
        assert_eq!(hunk_header_indices(&rf, ViewMode::Unified), vec![0]);
        assert_eq!(line_count(&rf, ViewMode::Unified), 2);
    }

    #[test]
    fn emphasis_membership() {
        let ranges = [(2usize, 5usize)];
        assert!(!in_ranges(1, &ranges));
        assert!(in_ranges(2, &ranges));
        assert!(in_ranges(4, &ranges));
        assert!(!in_ranges(5, &ranges));
    }

    /// Collect the rendered characters (spans flattened) for assertions.
    fn rendered_text(spans: &[Span]) -> String {
        spans.iter().flat_map(|s| s.content.chars()).collect()
    }

    #[test]
    fn tab_expands_to_next_tab_stop() {
        // A leading tab (width 4) becomes 4 spaces before the text.
        let spans = text_spans(
            "\tx",
            &[],
            &[],
            RowKind::Context,
            false,
            0,
            40,
            Palette::dark(),
            4,
        );
        let s = rendered_text(&spans);
        assert!(s.starts_with("    x"), "tab should expand to 4 spaces, got {s:?}");
    }

    #[test]
    fn tab_aligns_partial_column_to_stop() {
        // "ab\tc" with width 4: after "ab" (col 2) a tab fills 2 cells to col 4.
        let spans = text_spans(
            "ab\tc",
            &[],
            &[],
            RowKind::Context,
            false,
            0,
            40,
            Palette::dark(),
            4,
        );
        let s = rendered_text(&spans);
        assert!(s.starts_with("ab  c"), "expected alignment to next stop, got {s:?}");
    }

    #[test]
    fn tab_expands_within_syntax_segments() {
        // Segments (syntax path) carry the tab too; it must still expand.
        let segs = vec![(Style::default().fg(Color::Rgb(1, 2, 3)), "\tlet".to_string())];
        let spans = text_spans(
            "\tlet",
            &segs,
            &[],
            RowKind::Added,
            false,
            0,
            40,
            Palette::dark(),
            2,
        );
        let s = rendered_text(&spans);
        assert!(s.starts_with("  let"), "tab in segment should expand, got {s:?}");
    }

    #[test]
    fn renders_unified_buffer_with_header_and_text() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let rf = RenderedFile {
            diff: diff_with(vec![
                row(RowKind::Context, Some(1), Some(1), "let x = 1;"),
                row(RowKind::Added, None, Some(2), "let y = 2;"),
            ]),
            old_hl: vec![],
            new_hl: vec![],
        };

        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        terminal
            .draw(|f| {
                render(
                    f,
                    f.area(),
                    &rf,
                    &Marks::default(),
                    ViewMode::Unified,
                    0,
                    0,
                    0,
                    true,
                    Palette::dark(),
                    4,
                )
            })
            .unwrap();
        let rendered = format!("{}", terminal.backend());

        assert!(rendered.contains("@@"), "hunk header should render");
        assert!(rendered.contains("let x = 1;"), "context line should render");
        assert!(rendered.contains("let y = 2;"), "added line should render");
    }

    #[test]
    fn renders_side_by_side_without_panicking() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let rf = RenderedFile {
            diff: diff_with(vec![
                row(RowKind::Removed, Some(1), None, "old line"),
                row(RowKind::Added, None, Some(1), "new line"),
            ]),
            old_hl: vec![],
            new_hl: vec![],
        };

        let mut terminal = Terminal::new(TestBackend::new(120, 8)).unwrap();
        terminal
            .draw(|f| {
                render(
                    f,
                    f.area(),
                    &rf,
                    &Marks::default(),
                    ViewMode::SideBySide,
                    0,
                    0,
                    0,
                    true,
                    Palette::dark(),
                    4,
                )
            })
            .unwrap();
        let rendered = format!("{}", terminal.backend());
        assert!(rendered.contains("old line"));
        assert!(rendered.contains("new line"));
    }
}
