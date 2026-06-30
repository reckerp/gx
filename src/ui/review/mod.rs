//! The `gx review` TUI: the event loop, layout, and `Mode`/`Focus` dispatch
//! live here (U4); it composes the diff widget (U4), syntax highlighting (U3),
//! the file-tree sidebar (U5), and the comment overlay (U6) as units land.

pub mod diff_view;
pub mod highlight;

use crate::git::review::diff::ChangedFile;
use crate::git::review::range::ReviewRange;
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
    Help,
}

#[derive(PartialEq)]
enum Focus {
    #[allow(dead_code)] // Sidebar focus + j/k routing lands with the file tree (U5).
    Sidebar,
    Diff,
}

/// Launch the review TUI for an already-resolved range and changed-file list.
pub fn run(range: ReviewRange, files: Vec<ChangedFile>, theme: &str, min_width: u16) -> Result<()> {
    // `with_terminal` enters the alternate screen / raw mode and restores it
    // (even on panic, via its guard) before returning; the inner Result is the
    // review loop's own outcome.
    with_terminal(|terminal| run_loop(terminal, range, files, theme, min_width)).into_diagnostic()?
}

fn run_loop(
    terminal: &mut crate::ui::Term,
    range: ReviewRange,
    files: Vec<ChangedFile>,
    theme: &str,
    min_width: u16,
) -> Result<()> {
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
    Ok(())
}

struct App {
    range: ReviewRange,
    files: Vec<ChangedFile>,
    selected: usize,
    cache: Vec<Option<RenderedFile>>,
    highlighter: Highlighter,
    cursor: usize,
    v_scroll: usize,
    h_scroll: usize,
    view_override: Option<ViewMode>,
    min_width: u16,
    show_sidebar: bool,
    mode: Mode,
    focus: Focus,
    pending_bracket: Option<char>,
    // Last-rendered geometry, used by the key handler for paging/clamping.
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
            cursor: 0,
            v_scroll: 0,
            h_scroll: 0,
            view_override: None,
            min_width,
            show_sidebar: true,
            mode: Mode::Normal,
            focus: Focus::Diff,
            pending_bracket: None,
            last_diff_height: 1,
            last_view: ViewMode::SideBySide,
        }
    }

    /// Build (diff + highlight) the selected file on first view, then cache it.
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
        if matches!(self.mode, Mode::Help) {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                self.mode = Mode::Normal;
            }
            return false;
        }

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

            (KeyCode::Char('v'), _) => self.toggle_view(),
            (KeyCode::Char('b'), _) => self.show_sidebar = !self.show_sidebar,
            (KeyCode::Char('?'), _) => self.mode = Mode::Help,
            _ => {}
        }
        false
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

        if self.files.is_empty() {
            self.draw_empty(f, diff_area);
        } else if let Some(rf) = &self.cache[self.selected] {
            diff_view::render(
                f,
                diff_area,
                rf,
                view,
                self.cursor,
                self.v_scroll,
                self.h_scroll,
                self.focus == Focus::Diff,
            );
        }

        f.render_widget(self.help_bar(), help_area);
        if matches!(self.mode, Mode::Help) {
            self.draw_help_overlay(f, area);
        }
    }

    fn draw_empty(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Review — {} ", self.range.label));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = format!(
            "No changes in {}.\n\nPress q to quit.",
            self.range.label
        );
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            inner,
        );
    }

    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Files ({}) ", self.files.len()));
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
                Line::from(vec![
                    Span::styled(format!("{icon} "), Style::default().fg(color)),
                    Span::styled(name.to_string(), name_style),
                ])
            })
            .collect();

        f.render_widget(Paragraph::new(lines), inner);
    }

    fn help_bar(&self) -> Paragraph<'static> {
        render_help_bar(&[
            ("j/k", "move"),
            ("C-d/u", "page"),
            ("]c/[c", "hunk"),
            ("Tab", "file"),
            ("v", "view"),
            ("b", "sidebar"),
            ("?", "help"),
            ("q", "quit"),
        ])
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
            Line::raw("v              toggle split / unified"),
            Line::raw("b              toggle sidebar"),
            Line::raw("? / esc        close this help"),
            Line::raw("q              quit"),
        ];
        let popup = centered_rect(60, 70, area);
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
