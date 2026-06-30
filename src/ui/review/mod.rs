//! The `gx review` TUI: the event loop, layout, `Mode`/`Focus` dispatch, and
//! the comment overlay live here. It composes the diff widget + syntax
//! highlighting (U3/U4); the file-tree sidebar (U5) and persistence (U7) slot in
//! as those units land.

pub mod diff_view;
pub mod highlight;

use crate::git::review::blob;
use crate::git::review::diff::ChangedFile;
use crate::git::review::range::ReviewRange;
use crate::git::review::state::{Comment, Marks, ReviewState, Side};
use crate::ui::terminal::with_terminal;
use crate::ui::{render_help_bar, status_char, status_color};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use diff_view::{RenderedFile, ViewMode};
use highlight::Highlighter;
use miette::{IntoDiagnostic, Result};
use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::time::Duration;

const SIDEBAR_WIDTH: u16 = 32;
const SELECT_BG: Color = Color::Rgb(50, 50, 70);

enum Mode {
    Normal,
    VisualSelect,
    CommentPopup,
    Help,
}

#[derive(PartialEq)]
enum Focus {
    #[allow(dead_code)] // Sidebar focus + j/k routing lands with the file tree (U5).
    Sidebar,
    Diff,
}

/// In-progress comment being composed in the inline popup.
struct Popup {
    file: String,
    side: Side,
    start_line: usize,
    end_line: usize,
    anchor_text: String,
    buffer: String,
    /// Index of the comment being edited, or `None` for a new comment.
    editing: Option<usize>,
}

/// Launch the review TUI for an already-resolved range and changed-file list.
pub fn run(range: ReviewRange, files: Vec<ChangedFile>, theme: &str, min_width: u16) -> Result<()> {
    // `with_terminal` enters the alternate screen / raw mode and restores it
    // (even on panic, via its guard) before returning; the inner Result carries
    // the loop's outcome plus an optional message to print after teardown.
    let message = with_terminal(|terminal| run_loop(terminal, range, files, theme, min_width))
        .into_diagnostic()??;
    if let Some(msg) = message {
        println!("{msg}");
    }
    Ok(())
}

fn run_loop(
    terminal: &mut crate::ui::Term,
    range: ReviewRange,
    files: Vec<ChangedFile>,
    theme: &str,
    min_width: u16,
) -> Result<Option<String>> {
    let mut app = App::new(range, files, theme, min_width);

    loop {
        app.ensure_current_built()?;
        terminal.draw(|f| app.draw(f)).into_diagnostic()?;

        if event::poll(Duration::from_millis(100)).into_diagnostic()?
            && let Event::Key(key) = event::read().into_diagnostic()?
            && app.handle_key(key)
        {
            break;
        }
    }
    Ok(app.finish_message.take())
}

struct App {
    range: ReviewRange,
    files: Vec<ChangedFile>,
    selected: usize,
    cache: Vec<Option<RenderedFile>>,
    highlighter: Highlighter,
    review: ReviewState,
    cursor: usize,
    v_scroll: usize,
    h_scroll: usize,
    view_override: Option<ViewMode>,
    min_width: u16,
    show_sidebar: bool,
    mode: Mode,
    focus: Focus,
    popup: Option<Popup>,
    select_anchor: Option<usize>,
    status: Option<String>,
    finish_message: Option<String>,
    pending_bracket: Option<char>,
    last_diff_height: usize,
    last_view: ViewMode,
}

impl App {
    fn new(range: ReviewRange, files: Vec<ChangedFile>, theme: &str, min_width: u16) -> Self {
        let cache = (0..files.len()).map(|_| None).collect();
        App {
            range,
            files,
            selected: 0,
            cache,
            highlighter: Highlighter::new(theme),
            review: ReviewState::default(),
            cursor: 0,
            v_scroll: 0,
            h_scroll: 0,
            view_override: None,
            min_width,
            show_sidebar: true,
            mode: Mode::Normal,
            focus: Focus::Diff,
            popup: None,
            select_anchor: None,
            status: None,
            finish_message: None,
            pending_bracket: None,
            last_diff_height: 1,
            last_view: ViewMode::SideBySide,
        }
    }

    fn ensure_current_built(&mut self) -> Result<()> {
        if self.files.is_empty() || self.cache[self.selected].is_some() {
            return Ok(());
        }
        let rf = diff_view::render_file(
            &self.files[self.selected],
            self.range.to,
            &self.highlighter,
        )
        .into_diagnostic()?;
        self.cache[self.selected] = Some(rf);
        Ok(())
    }

    fn current(&self) -> Option<&RenderedFile> {
        self.cache.get(self.selected).and_then(|o| o.as_ref())
    }

    fn current_path(&self) -> Option<&str> {
        self.files.get(self.selected).map(|f| f.path.as_str())
    }

    fn cur_line_count(&self) -> usize {
        self.current()
            .map(|rf| diff_view::line_count(rf, self.last_view))
            .unwrap_or(0)
    }

    fn view_mode(&self, width: u16) -> ViewMode {
        if let Some(v) = self.view_override {
            return v;
        }
        if width < self.min_width {
            ViewMode::Unified
        } else {
            ViewMode::SideBySide
        }
    }

    // --- input ------------------------------------------------------------

    /// Returns true when the app should quit.
    fn handle_key(&mut self, key: event::KeyEvent) -> bool {
        self.status = None;
        match self.mode {
            Mode::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
                ) {
                    self.mode = Mode::Normal;
                }
                false
            }
            Mode::CommentPopup => {
                self.handle_popup_key(key);
                false
            }
            Mode::VisualSelect => {
                self.handle_visual_key(key);
                false
            }
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: event::KeyEvent) -> bool {
        // Resolve a pending `]`/`[` chord (vim diff motions ]c / [c).
        if let Some(bracket) = self.pending_bracket.take()
            && key.code == KeyCode::Char('c')
        {
            match bracket {
                ']' => self.jump_hunk(true),
                '[' => self.jump_hunk(false),
                _ => {}
            }
            return false;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _)
            | (KeyCode::Esc, _)
            | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,

            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => self.move_cursor(1),
            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => self.move_cursor(-1),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => self.move_cursor(self.page()),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => self.move_cursor(-self.page()),
            (KeyCode::Char('g'), _) | (KeyCode::Home, _) => {
                self.cursor = 0;
                self.ensure_cursor_visible();
            }
            (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
                self.cursor = self.cur_line_count().saturating_sub(1);
                self.ensure_cursor_visible();
            }

            (KeyCode::Char(']'), _) => self.pending_bracket = Some(']'),
            (KeyCode::Char('['), _) => self.pending_bracket = Some('['),
            (KeyCode::Char('}'), _) => self.jump_hunk(true),
            (KeyCode::Char('{'), _) => self.jump_hunk(false),

            (KeyCode::Tab, _) => self.switch_file(1),
            (KeyCode::BackTab, _) => self.switch_file(-1),

            (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
                self.h_scroll = self.h_scroll.saturating_sub(4)
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => self.h_scroll += 4,

            // Comments.
            (KeyCode::Char('c'), KeyModifiers::NONE) => self.start_comment_current(),
            (KeyCode::Char('V'), _) => {
                self.select_anchor = Some(self.cursor);
                self.mode = Mode::VisualSelect;
            }
            (KeyCode::Enter, _) => self.edit_comment_under_cursor(),
            (KeyCode::Char('D'), _) => self.delete_comment_under_cursor(),
            (KeyCode::Char('F'), _) => return self.finish(),

            (KeyCode::Char('v'), _) => self.toggle_view(),
            (KeyCode::Char('b'), _) => self.show_sidebar = !self.show_sidebar,
            (KeyCode::Char('?'), _) => self.mode = Mode::Help,
            _ => {}
        }
        false
    }

    fn handle_visual_key(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.select_anchor = None;
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('c') => self.start_comment_selection(),
            _ => {}
        }
    }

    fn handle_popup_key(&mut self, key: event::KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.popup = None;
                self.mode = Mode::Normal;
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => self.save_comment(),
            (KeyCode::Enter, _) => {
                if let Some(p) = self.popup.as_mut() {
                    p.buffer.push('\n');
                }
            }
            (KeyCode::Backspace, _) => {
                if let Some(p) = self.popup.as_mut() {
                    p.buffer.pop();
                }
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                if let Some(p) = self.popup.as_mut() {
                    p.buffer.push(c);
                }
            }
            _ => {}
        }
    }

    fn page(&self) -> i32 {
        (self.last_diff_height / 2).max(1) as i32
    }

    fn move_cursor(&mut self, delta: i32) {
        let count = self.cur_line_count();
        if count == 0 {
            return;
        }
        let max = (count - 1) as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(0, max) as usize;
        self.ensure_cursor_visible();
    }

    fn ensure_cursor_visible(&mut self) {
        let h = self.last_diff_height.max(1);
        if self.cursor < self.v_scroll {
            self.v_scroll = self.cursor;
        } else if self.cursor >= self.v_scroll + h {
            self.v_scroll = self.cursor + 1 - h;
        }
    }

    fn jump_hunk(&mut self, forward: bool) {
        let Some(rf) = self.current() else { return };
        let headers = diff_view::hunk_header_indices(rf, self.last_view);
        let target = if forward {
            headers.iter().find(|&&i| i > self.cursor).copied()
        } else {
            headers.iter().rev().find(|&&i| i < self.cursor).copied()
        };
        if let Some(t) = target {
            self.cursor = t;
            self.ensure_cursor_visible();
        }
    }

    fn switch_file(&mut self, delta: i32) {
        if self.files.is_empty() {
            return;
        }
        let n = self.files.len() as i32;
        self.selected = (((self.selected as i32 + delta) % n + n) % n) as usize;
        self.cursor = 0;
        self.v_scroll = 0;
        self.h_scroll = 0;
    }

    fn toggle_view(&mut self) {
        self.view_override = Some(match self.last_view {
            ViewMode::SideBySide => ViewMode::Unified,
            ViewMode::Unified => ViewMode::SideBySide,
        });
    }

    // --- comments ---------------------------------------------------------

    fn start_comment_current(&mut self) {
        let anchor = self
            .current()
            .and_then(|rf| diff_view::anchor_at(rf, self.last_view, self.cursor));
        match anchor {
            Some(a) => self.open_popup(a.side, a.line, a.line, a.text),
            None => self.status = Some("Can't comment on a hunk header / gap line".into()),
        }
    }

    fn start_comment_selection(&mut self) {
        let lo = self.select_anchor.unwrap_or(self.cursor);
        let span = self
            .current()
            .and_then(|rf| diff_view::anchor_span(rf, self.last_view, lo, self.cursor));
        self.select_anchor = None;
        match span {
            Some((side, start, end, text)) => self.open_popup(side, start, end, text),
            None => {
                self.mode = Mode::Normal;
                self.status = Some("Nothing to comment in selection".into());
            }
        }
    }

    fn open_popup(&mut self, side: Side, start: usize, end: usize, anchor_text: String) {
        let Some(file) = self.current_path().map(str::to_string) else {
            return;
        };
        self.popup = Some(Popup {
            file,
            side,
            start_line: start,
            end_line: end,
            anchor_text,
            buffer: String::new(),
            editing: None,
        });
        self.mode = Mode::CommentPopup;
    }

    fn edit_comment_under_cursor(&mut self) {
        let anchor = self
            .current()
            .and_then(|rf| diff_view::anchor_at(rf, self.last_view, self.cursor));
        let Some(a) = anchor else { return };
        let Some(file) = self.current_path().map(str::to_string) else {
            return;
        };
        let Some(idx) = self.review.index_at(&file, a.side, a.line) else {
            self.status = Some("No comment on this line".into());
            return;
        };
        let c = &self.review.comments[idx];
        self.popup = Some(Popup {
            file,
            side: c.side,
            start_line: c.start_line,
            end_line: c.end_line,
            anchor_text: c.anchor_text.clone(),
            buffer: c.body.clone(),
            editing: Some(idx),
        });
        self.mode = Mode::CommentPopup;
    }

    fn delete_comment_under_cursor(&mut self) {
        let anchor = self
            .current()
            .and_then(|rf| diff_view::anchor_at(rf, self.last_view, self.cursor));
        let Some(a) = anchor else { return };
        let Some(file) = self.current_path().map(str::to_string) else {
            return;
        };
        if let Some(idx) = self.review.index_at(&file, a.side, a.line) {
            self.review.remove(idx);
            self.status = Some("Comment deleted".into());
        }
    }

    fn save_comment(&mut self) {
        let Some(popup) = self.popup.take() else { return };
        self.mode = Mode::Normal;
        if popup.buffer.trim().is_empty() {
            self.status = Some("Empty comment discarded".into());
            return;
        }
        match popup.editing {
            Some(idx) => self.review.set_body(idx, popup.buffer),
            None => self.review.add(Comment {
                file: popup.file,
                side: popup.side,
                start_line: popup.start_line,
                end_line: popup.end_line,
                anchor_text: popup.anchor_text,
                body: popup.buffer,
            }),
        }
    }

    /// Build the wrapped review blob, copy it to the clipboard, and signal quit.
    /// Returns false (does not quit) when there is nothing to finish.
    fn finish(&mut self) -> bool {
        let total = self.review.total();
        if total == 0 {
            self.status = Some("No comments yet — nothing to finish".into());
            return false;
        }

        // Gather blocks in (file, line) order; the inner scope drops the
        // borrows of `review`/`files`/`cache` before we mutate `finish_message`.
        let blocks = {
            let mut comments: Vec<&Comment> = self.review.comments.iter().collect();
            comments.sort_by(|a, b| a.file.cmp(&b.file).then(a.start_line.cmp(&b.start_line)));
            comments
                .iter()
                .map(|c| {
                    let snippet = self
                        .files
                        .iter()
                        .position(|f| f.path == c.file)
                        .and_then(|i| self.cache[i].as_ref())
                        .map(|rf| blob::context_lines(&rf.diff, c.side, c.start_line, c.end_line))
                        .unwrap_or_default();
                    blob::CommentBlock {
                        file: c.file.clone(),
                        location: blob::location(c.side, c.start_line, c.end_line),
                        snippet,
                        body: c.body.clone(),
                    }
                })
                .collect::<Vec<_>>()
        };

        let text = blob::build(&self.range.label, &blocks);
        self.finish_message = Some(match crate::clipboard::copy(&text) {
            Ok(()) => format!("✓ Copied review ({total} comment(s)) to the clipboard."),
            Err(_) => format!("Clipboard tool unavailable — here is the review blob:\n\n{text}"),
        });
        true
    }

    // --- rendering --------------------------------------------------------

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(area);
        let main = chunks[0];
        let help_area = chunks[1];

        let diff_area = if self.show_sidebar {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)])
                .split(main);
            self.draw_sidebar(f, cols[0]);
            cols[1]
        } else {
            main
        };

        let view = self.view_mode(diff_area.width);
        self.last_view = view;
        self.last_diff_height = diff_area.height.saturating_sub(2) as usize;

        let marks = self
            .current_path()
            .map(|p| self.review.marks_for(p))
            .unwrap_or_default();

        if self.files.is_empty() {
            self.draw_empty(f, diff_area);
        } else if let Some(rf) = &self.cache[self.selected] {
            diff_view::render(
                f,
                diff_area,
                rf,
                &marks,
                view,
                self.cursor,
                self.v_scroll,
                self.h_scroll,
                self.focus == Focus::Diff,
            );
        }

        f.render_widget(self.help_bar(), help_area);

        match self.mode {
            Mode::Help => self.draw_help_overlay(f, area),
            Mode::CommentPopup => self.draw_popup(f, area),
            _ => {}
        }
    }

    fn draw_empty(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Review — {} ", self.range.label));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = format!("No changes in {}.\n\nPress q to quit.", self.range.label);
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            inner,
        );
    }

    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let total = self.review.total();
        let title = if total > 0 {
            format!(" Files ({}) · {} 💬 ", self.files.len(), total)
        } else {
            format!(" Files ({}) ", self.files.len())
        };
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let h = inner.height as usize;
        let start = if self.selected >= h {
            self.selected - h + 1
        } else {
            0
        };

        let lines: Vec<Line> = self
            .files
            .iter()
            .enumerate()
            .skip(start)
            .take(h)
            .map(|(i, file)| {
                let icon = status_char(file.status);
                let color = status_color(file.status);
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                let name_style = if i == self.selected {
                    Style::default().bg(SELECT_BG).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let mut spans = vec![
                    Span::styled(format!("{icon} "), Style::default().fg(color)),
                    Span::styled(name.to_string(), name_style),
                ];
                let count = self.review.count_for_file(&file.path);
                if count > 0 {
                    spans.push(Span::styled(
                        format!(" ({count})"),
                        Style::default().fg(Color::Magenta),
                    ));
                }
                Line::from(spans)
            })
            .collect();

        f.render_widget(Paragraph::new(lines), inner);
    }

    fn help_bar(&self) -> Paragraph<'static> {
        if let Some(status) = &self.status {
            return Paragraph::new(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(Color::Yellow),
            )))
            .block(Block::default().borders(Borders::ALL).title(" Status "));
        }
        let hints: &[(&str, &str)] = match self.mode {
            Mode::Normal => &[
                ("j/k", "move"),
                ("c", "comment"),
                ("V", "multi"),
                ("⏎", "edit"),
                ("D", "del"),
                ("]c", "hunk"),
                ("Tab", "file"),
                ("v", "view"),
                ("F", "finish"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Mode::VisualSelect => &[("j/k", "extend"), ("c", "comment"), ("esc", "cancel")],
            Mode::CommentPopup => &[
                ("type", "comment"),
                ("C-s", "save"),
                ("⏎", "newline"),
                ("esc", "cancel"),
            ],
            Mode::Help => &[("esc", "close")],
        };
        render_help_bar(hints)
    }

    fn draw_popup(&self, f: &mut Frame, area: Rect) {
        let Some(p) = &self.popup else { return };
        let name = p.file.rsplit('/').next().unwrap_or(&p.file);
        let lines = if p.end_line > p.start_line {
            format!("{}-{}", p.start_line, p.end_line)
        } else {
            p.start_line.to_string()
        };
        let verb = if p.editing.is_some() { "Edit" } else { "Comment" };
        let title = format!(" {verb} {name}:{lines} ");

        let popup_area = centered_rect(60, 50, area);
        f.render_widget(Clear, popup_area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title);
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        let body_help = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        let mut text = p.buffer.clone();
        text.push('▏'); // simple insertion caret
        f.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: false }),
            body_help[0],
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Ctrl-s save · ⏎ newline · esc cancel",
                Style::default().fg(Color::DarkGray),
            ))),
            body_help[1],
        );
    }

    fn draw_help_overlay(&self, f: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(Span::styled(
                "gx review — keys",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::raw("j / k          move cursor"),
            Line::raw("Ctrl-d / -u    half page down / up"),
            Line::raw("g / G          top / bottom"),
            Line::raw("]c / [c        next / prev hunk  (also } / {)"),
            Line::raw("Tab / S-Tab    next / prev file"),
            Line::raw("h / l  ← →     scroll horizontally"),
            Line::raw("c              comment on the current line"),
            Line::raw("V              start a multi-line selection, then c"),
            Line::raw("⏎              edit the comment under the cursor"),
            Line::raw("D              delete the comment under the cursor"),
            Line::raw("F              finish: copy the review to the clipboard"),
            Line::raw("v              toggle split / unified"),
            Line::raw("b              toggle sidebar"),
            Line::raw("? / esc        close this help"),
            Line::raw("q              quit"),
        ];
        let popup = centered_rect(60, 80, area);
        f.render_widget(Clear, popup);
        let block = Block::default().borders(Borders::ALL).title(" Help ");
        f.render_widget(Paragraph::new(lines).block(block), popup);
    }
}

/// A centered rectangle `percent_x`% × `percent_y`% of `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
