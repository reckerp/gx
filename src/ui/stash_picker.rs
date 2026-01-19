use super::{Term, render_help_bar};
use crate::git::stash::StashEntry;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use miette::IntoDiagnostic;
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StashAction {
    Pop,
    Apply,
    Drop,
    Show,
    Branch,
}

pub struct StashPickerResult {
    pub entry: StashEntry,
    pub action: StashAction,
}

struct ActionMenu {
    actions: Vec<StashAction>,
    selected: usize,
}

impl ActionMenu {
    fn new() -> Self {
        Self {
            actions: vec![
                StashAction::Pop,
                StashAction::Apply,
                StashAction::Drop,
                StashAction::Show,
                StashAction::Branch,
            ],
            selected: 0,
        }
    }

    fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn down(&mut self) {
        if self.selected + 1 < self.actions.len() {
            self.selected += 1;
        }
    }

    fn selected_action(&self) -> StashAction {
        self.actions[self.selected]
    }
}

fn action_label(action: StashAction) -> &'static str {
    match action {
        StashAction::Pop => "Pop (apply & remove)",
        StashAction::Apply => "Apply (keep stash)",
        StashAction::Drop => "Drop (delete)",
        StashAction::Show => "Show (view diff)",
        StashAction::Branch => "Branch (create from stash)",
    }
}

fn action_color(action: StashAction) -> Color {
    match action {
        StashAction::Pop => Color::Green,
        StashAction::Apply => Color::Cyan,
        StashAction::Drop => Color::Red,
        StashAction::Show => Color::Yellow,
        StashAction::Branch => Color::Magenta,
    }
}

#[derive(PartialEq)]
enum Mode {
    List,
    Action,
}

pub fn run(
    terminal: &mut Term,
    stashes: &[StashEntry],
) -> miette::Result<Option<StashPickerResult>> {
    if stashes.is_empty() {
        return Ok(None);
    }

    let mut selected_index = 0;
    let mut mode = Mode::List;
    let mut action_menu = ActionMenu::new();

    loop {
        terminal
            .draw(|f| {
                let area = f.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)])
                    .split(area);

                match mode {
                    Mode::List => render_list(f, chunks[0], stashes, selected_index),
                    Mode::Action => {
                        render_action_menu(f, chunks[0], stashes, selected_index, &action_menu)
                    }
                }

                let help = match mode {
                    Mode::List => render_help_bar(&[
                        ("j/k", "navigate"),
                        ("enter", "actions"),
                        ("p", "pop"),
                        ("a", "apply"),
                        ("d", "drop"),
                        ("esc", "quit"),
                    ]),
                    Mode::Action => render_help_bar(&[
                        ("j/k", "navigate"),
                        ("enter", "confirm"),
                        ("esc", "back"),
                    ]),
                };
                f.render_widget(help, chunks[1]);
            })
            .into_diagnostic()?;

        if event::poll(Duration::from_millis(50)).into_diagnostic()?
            && let Event::Key(key) = event::read().into_diagnostic()?
        {
            match mode {
                Mode::List => match (key.code, key.modifiers) {
                    (KeyCode::Esc, _)
                    | (KeyCode::Char('q'), _)
                    | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        return Ok(None);
                    }
                    (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                        selected_index = selected_index.saturating_sub(1);
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                        if selected_index + 1 < stashes.len() {
                            selected_index += 1;
                        }
                    }
                    (KeyCode::Enter, _) => {
                        mode = Mode::Action;
                        action_menu = ActionMenu::new();
                    }
                    (KeyCode::Char('p'), _) => {
                        return Ok(Some(StashPickerResult {
                            entry: stashes[selected_index].clone(),
                            action: StashAction::Pop,
                        }));
                    }
                    (KeyCode::Char('a'), _) => {
                        return Ok(Some(StashPickerResult {
                            entry: stashes[selected_index].clone(),
                            action: StashAction::Apply,
                        }));
                    }
                    (KeyCode::Char('d'), _) => {
                        return Ok(Some(StashPickerResult {
                            entry: stashes[selected_index].clone(),
                            action: StashAction::Drop,
                        }));
                    }
                    (KeyCode::Char('s'), _) => {
                        return Ok(Some(StashPickerResult {
                            entry: stashes[selected_index].clone(),
                            action: StashAction::Show,
                        }));
                    }
                    (KeyCode::Char('b'), _) => {
                        return Ok(Some(StashPickerResult {
                            entry: stashes[selected_index].clone(),
                            action: StashAction::Branch,
                        }));
                    }
                    _ => {}
                },
                Mode::Action => match (key.code, key.modifiers) {
                    (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        mode = Mode::List;
                    }
                    (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                        action_menu.up();
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                        action_menu.down();
                    }
                    (KeyCode::Enter, _) => {
                        return Ok(Some(StashPickerResult {
                            entry: stashes[selected_index].clone(),
                            action: action_menu.selected_action(),
                        }));
                    }
                    _ => {}
                },
            }
        }
    }
}

fn render_list(f: &mut ratatui::Frame, area: Rect, stashes: &[StashEntry], selected: usize) {
    let items: Vec<ListItem> = stashes
        .iter()
        .enumerate()
        .map(|(i, stash)| {
            let is_current = i == selected;

            let line = Line::from(vec![
                Span::styled(
                    format!("stash@{{{}}}", stash.index),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("[{}]", stash.branch),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" "),
                Span::styled(
                    truncate_message(&stash.message, 50),
                    if is_current {
                        Style::default().fg(Color::White).bold()
                    } else {
                        Style::default()
                    },
                ),
                Span::raw(" "),
                Span::styled(&stash.time_relative, Style::default().fg(Color::DarkGray)),
            ]);

            if is_current {
                ListItem::new(line).style(Style::default().bg(Color::DarkGray))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let title = format!(" Stashes ({}) ", stashes.len());
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(list, area);
}

fn render_action_menu(
    f: &mut ratatui::Frame,
    area: Rect,
    stashes: &[StashEntry],
    stash_index: usize,
    menu: &ActionMenu,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    render_list(f, chunks[0], stashes, stash_index);

    let items: Vec<ListItem> = menu
        .actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let is_selected = i == menu.selected;
            let style = if is_selected {
                Style::default().fg(Color::Black).bg(action_color(*action))
            } else {
                Style::default().fg(action_color(*action))
            };
            ListItem::new(action_label(*action)).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Select Action "),
    );

    f.render_widget(list, chunks[1]);
}

fn truncate_message(msg: &str, max_len: usize) -> String {
    let first_line = msg.lines().next().unwrap_or(msg);
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len - 3])
    } else {
        first_line.to_string()
    }
}
