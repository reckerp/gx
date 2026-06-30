use super::{TermStderr, adjust_scroll, render_help_bar};
use crate::git::time::format_relative;
use crate::git::worktree::{
    SummaryLookup, Worktree, WorktreeSummary, apply_local_summaries, pending_summaries,
};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

/// Why a row cannot be selected for removal. Surfaced in the UI so the user
/// understands the cleaner did not simply miss the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisabledReason {
    Main,
    Current,
    Locked,
    Protected,
}

impl DisabledReason {
    fn tag(&self) -> &'static str {
        match self {
            DisabledReason::Main => "(main)",
            DisabledReason::Current => "(current)",
            DisabledReason::Locked => "(locked)",
            DisabledReason::Protected => "(protected)",
        }
    }
}

/// A single line in the multi-section cleaner. Section headers are
/// non-selectable; the remaining variants each carry their selectable state.
#[derive(Debug, Clone)]
pub enum CleanRow {
    Header(&'static str),
    /// A workspace (worktree).
    Workspace {
        worktree: Worktree,
        disabled: Option<DisabledReason>,
        /// Precomputed age in whole days, or `None` when it could not be
        /// resolved. Computed once when rows are built so the render loop does
        /// not re-run repository discovery + a revparse per row per frame.
        age_days: Option<u64>,
    },
    /// A local branch with no worktree.
    OrphanBranch {
        name: String,
        has_unpushed: bool,
        has_upstream: bool,
        disabled: Option<DisabledReason>,
    },
    /// A local branch whose upstream tracking branch is gone.
    GoneBranch {
        name: String,
        disabled: Option<DisabledReason>,
    },
}

impl CleanRow {
    fn is_header(&self) -> bool {
        matches!(self, CleanRow::Header(_))
    }

    fn disabled_reason(&self) -> Option<&DisabledReason> {
        match self {
            CleanRow::Header(_) => None,
            CleanRow::Workspace { disabled, .. }
            | CleanRow::OrphanBranch { disabled, .. }
            | CleanRow::GoneBranch { disabled, .. } => disabled.as_ref(),
        }
    }

    /// A stable key for selection tracking. Workspaces key on their path,
    /// branches on a `branch:` prefix so a branch and a same-named directory
    /// never collide.
    fn selection_key(&self) -> Option<String> {
        match self {
            CleanRow::Header(_) => None,
            CleanRow::Workspace { worktree, .. } => Some(format!("ws:{}", worktree.path.display())),
            CleanRow::OrphanBranch { name, .. } | CleanRow::GoneBranch { name, .. } => {
                Some(format!("branch:{}", name))
            }
        }
    }

    fn is_selectable(&self) -> bool {
        !self.is_header() && self.disabled_reason().is_none()
    }
}

/// Inputs the picker needs that depend on protection/safety decisions made by
/// the caller (which owns the config). Kept as plain data so [`build_rows`] is a
/// pure, unit-testable function.
pub struct CleanInputs {
    pub worktrees: Vec<Worktree>,
    /// Local branches with no worktree (name, has_unpushed, has_upstream).
    pub orphan_branches: Vec<(String, bool, bool)>,
    /// Branches whose upstream tracking branch is gone.
    pub gone_branches: Vec<String>,
    /// Branch names that are protected (never removable).
    pub protected: HashSet<String>,
    /// Precomputed workspace age in whole days, keyed by worktree path. Computed
    /// once by the caller so the render loop never recomputes it per frame; a
    /// missing entry renders as "age?".
    pub ages: HashMap<PathBuf, u64>,
}

/// The user's confirmed cleanup choices, consumed by the command handler.
#[derive(Debug, Clone, Default)]
pub struct CleanAction {
    pub remove_worktrees: Vec<Worktree>,
    pub delete_branches: Vec<String>,
    pub confirmed: bool,
}

/// Build the ordered, sectioned row list from already-resolved inputs. Pure so
/// the sectioning and safety-tagging is unit-testable; the TUI loop is not.
pub fn build_rows(inputs: &CleanInputs) -> Vec<CleanRow> {
    let mut rows = Vec::new();

    rows.push(CleanRow::Header("Workspaces"));
    for worktree in &inputs.worktrees {
        let disabled = workspace_disabled_reason(worktree, &inputs.protected);
        rows.push(CleanRow::Workspace {
            worktree: worktree.clone(),
            disabled,
            age_days: inputs.ages.get(&worktree.path).copied(),
        });
    }

    rows.push(CleanRow::Header("Local branches without workspaces"));
    for (name, has_unpushed, has_upstream) in &inputs.orphan_branches {
        let disabled = inputs
            .protected
            .contains(name)
            .then_some(DisabledReason::Protected);
        rows.push(CleanRow::OrphanBranch {
            name: name.clone(),
            has_unpushed: *has_unpushed,
            has_upstream: *has_upstream,
            disabled,
        });
    }

    rows.push(CleanRow::Header(
        "Orphan branches whose remote tracking branch is gone",
    ));
    for name in &inputs.gone_branches {
        let disabled = inputs
            .protected
            .contains(name)
            .then_some(DisabledReason::Protected);
        rows.push(CleanRow::GoneBranch {
            name: name.clone(),
            disabled,
        });
    }

    rows
}

/// Why a workspace row cannot be removed. Order matters only for the displayed
/// tag; all of these block removal.
fn workspace_disabled_reason(
    worktree: &Worktree,
    protected: &HashSet<String>,
) -> Option<DisabledReason> {
    if worktree.is_main {
        Some(DisabledReason::Main)
    } else if worktree.is_current {
        Some(DisabledReason::Current)
    } else if worktree.is_locked {
        Some(DisabledReason::Locked)
    } else if worktree
        .branch
        .as_deref()
        .is_some_and(|b| protected.contains(b))
    {
        Some(DisabledReason::Protected)
    } else {
        None
    }
}

/// Selection keys for every selectable row, in display order. Used by select-all.
fn selectable_keys(rows: &[CleanRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.is_selectable())
        .filter_map(|r| r.selection_key())
        .collect()
}

/// Resolve the user's selection into the concrete cleanup action.
fn build_action(rows: &[CleanRow], selected_keys: &HashSet<String>) -> CleanAction {
    let mut action = CleanAction::default();
    // Two rows can share a branch name (a `[gone]` branch with no worktree fits
    // both branch sections); dedupe so a branch is never deleted twice.
    let mut seen_branches: HashSet<String> = HashSet::new();

    for row in rows {
        let Some(key) = row.selection_key() else {
            continue;
        };
        if !selected_keys.contains(&key) {
            continue;
        }
        match row {
            CleanRow::Workspace { worktree, .. } => {
                action.remove_worktrees.push(worktree.clone());
            }
            CleanRow::OrphanBranch { name, .. } | CleanRow::GoneBranch { name, .. } => {
                if seen_branches.insert(name.clone()) {
                    action.delete_branches.push(name.clone());
                }
            }
            CleanRow::Header(_) => {}
        }
    }

    action
}

fn age_label(age_days: Option<u64>) -> String {
    match age_days {
        Some(days) => format_relative(days as i64 * 86_400),
        None => "age?".to_string(),
    }
}

fn render_row<'a>(
    row: &'a CleanRow,
    is_highlighted: bool,
    is_selected: bool,
    summaries: &HashMap<PathBuf, WorktreeSummary>,
) -> ListItem<'a> {
    if let CleanRow::Header(title) = row {
        return ListItem::new(Line::from(Span::styled(
            format!("── {} ──", title),
            Style::default().fg(Color::Cyan).bold(),
        )));
    }

    let disabled = row.disabled_reason();
    let checkbox = if disabled.is_some() {
        " - "
    } else if is_selected {
        "[x]"
    } else {
        "[ ]"
    };

    let name_style = if disabled.is_some() {
        Style::default().fg(Color::DarkGray)
    } else if is_highlighted {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::White)
    };

    let mut spans = vec![Span::styled(
        format!("  {} ", checkbox),
        if is_selected {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )];

    match row {
        CleanRow::Workspace {
            worktree, age_days, ..
        } => {
            spans.push(Span::styled(worktree.name.clone(), name_style));
            if let Some(branch) = &worktree.branch {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("[{}]", branch),
                    Style::default().fg(Color::Cyan),
                ));
            }
            spans.push(Span::styled(
                format!("  {}", age_label(*age_days)),
                Style::default().fg(Color::DarkGray),
            ));
            if let Some(summary) = summaries.get(&worktree.path) {
                spans.extend(badges(summary));
            }
            if worktree.is_main {
                spans.push(Span::styled(" (main)", Style::default().fg(Color::Green)));
            }
            if worktree.is_current {
                spans.push(Span::styled(
                    " (current)",
                    Style::default().fg(Color::Magenta),
                ));
            }
            if worktree.is_locked {
                spans.push(Span::styled(" (locked)", Style::default().fg(Color::Red)));
            }
        }
        CleanRow::OrphanBranch {
            name,
            has_unpushed,
            has_upstream,
            ..
        } => {
            spans.push(Span::styled(name.clone(), name_style));
            if !has_upstream {
                spans.push(Span::styled(
                    " (no upstream)",
                    Style::default().fg(Color::Yellow),
                ));
            } else if *has_unpushed {
                spans.push(Span::styled(" (unpushed)", Style::default().fg(Color::Red)));
            }
        }
        CleanRow::GoneBranch { name, .. } => {
            spans.push(Span::styled(name.clone(), name_style));
            spans.push(Span::styled(
                " (upstream gone)",
                Style::default().fg(Color::Yellow),
            ));
        }
        CleanRow::Header(_) => {}
    }

    if let Some(reason) = disabled {
        spans.push(Span::styled(
            format!(" {}", reason.tag()),
            Style::default().fg(Color::DarkGray),
        ));
    }

    let line = Line::from(spans);
    if is_highlighted {
        ListItem::new(line).style(Style::default().bg(Color::DarkGray))
    } else {
        ListItem::new(line)
    }
}

fn badges(summary: &WorktreeSummary) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    if summary.tracked_changes > 0 {
        out.push(Span::styled(
            format!(" dirty:{}", summary.tracked_changes),
            Style::default().fg(Color::Yellow),
        ));
    }
    if summary.untracked_changes > 0 {
        out.push(Span::styled(
            format!(" untracked:{}", summary.untracked_changes),
            Style::default().fg(Color::Red),
        ));
    }
    if let Some(ahead) = summary.ahead
        && ahead > 0
    {
        out.push(Span::styled(
            format!(" +{}", ahead),
            Style::default().fg(Color::Green),
        ));
    }
    if summary.status_error {
        out.push(Span::styled(
            " status?".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    } else if !summary.status_loaded {
        out.push(Span::styled(
            " status...".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    out
}

/// Move the highlight to the next selectable row in `direction` (+1/-1),
/// skipping headers and disabled rows. Returns the new index (unchanged when no
/// other selectable row exists).
fn next_selectable(rows: &[CleanRow], current: usize, direction: isize) -> usize {
    let len = rows.len() as isize;
    let mut idx = current as isize;
    loop {
        idx += direction;
        if idx < 0 || idx >= len {
            return current;
        }
        if rows[idx as usize].is_selectable() {
            return idx as usize;
        }
    }
}

fn first_selectable(rows: &[CleanRow]) -> usize {
    rows.iter().position(CleanRow::is_selectable).unwrap_or(0)
}

#[derive(Clone)]
enum Mode {
    List,
    Confirm,
}

/// Run the interactive multi-section cleaner. Returns `None` when the user
/// cancels, or a confirmed [`CleanAction`] otherwise (an empty action means the
/// user confirmed with nothing selected, which the caller treats as a no-op).
pub fn run(
    terminal: &mut TermStderr,
    inputs: CleanInputs,
    summary_lookup: SummaryLookup,
) -> miette::Result<Option<CleanAction>> {
    use miette::IntoDiagnostic;

    let rows = build_rows(&inputs);
    let mut summaries = pending_summaries(&inputs.worktrees);

    let mut highlighted = first_selectable(&rows);
    let mut scroll_offset = 0;
    let mut selected_keys: HashSet<String> = HashSet::new();
    let mut mode = Mode::List;

    let nothing_selectable = !rows.iter().any(CleanRow::is_selectable);

    loop {
        if let Ok(lookup) = summary_lookup.try_recv() {
            apply_local_summaries(&mut summaries, lookup);
        }

        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)])
                    .split(f.area());

                match mode {
                    Mode::List => {
                        let visible_height = chunks[0].height.saturating_sub(2) as usize;
                        scroll_offset = adjust_scroll(highlighted, scroll_offset, visible_height);

                        let items: Vec<ListItem> = rows
                            .iter()
                            .enumerate()
                            .skip(scroll_offset)
                            .take(visible_height)
                            .map(|(i, row)| {
                                let is_selected = row
                                    .selection_key()
                                    .is_some_and(|k| selected_keys.contains(&k));
                                render_row(row, i == highlighted, is_selected, &summaries)
                            })
                            .collect();

                        let title = format!(" Clean ({} selected) ", selected_keys.len());
                        f.render_widget(
                            List::new(items)
                                .block(Block::default().borders(Borders::ALL).title(title)),
                            chunks[0],
                        );
                        f.render_widget(
                            render_help_bar(&[
                                ("^/k", "Up"),
                                ("v/j", "Down"),
                                ("Space", "Select"),
                                ("^a", "All"),
                                ("^u", "Clear"),
                                ("Enter", "Clean"),
                                ("Esc", "Cancel"),
                            ]),
                            chunks[1],
                        );
                    }
                    Mode::Confirm => {
                        let action = build_action(&rows, &selected_keys);
                        f.render_widget(render_confirm(&action), chunks[0]);
                        f.render_widget(
                            render_help_bar(&[("Enter/y", "Confirm"), ("Esc/n", "Back")]),
                            chunks[1],
                        );
                    }
                }
            })
            .into_diagnostic()?;

        if !event::poll(Duration::from_millis(50)).into_diagnostic()? {
            continue;
        }
        let Event::Key(key) = event::read().into_diagnostic()? else {
            continue;
        };

        match mode {
            Mode::List => match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                    highlighted = next_selectable(&rows, highlighted, -1);
                }
                (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                    highlighted = next_selectable(&rows, highlighted, 1);
                }
                (KeyCode::Char(' '), _) => {
                    if let Some(row) = rows.get(highlighted)
                        && row.is_selectable()
                        && let Some(key) = row.selection_key()
                        && !selected_keys.insert(key.clone())
                    {
                        selected_keys.remove(&key);
                    }
                }
                (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                    let keys = selectable_keys(&rows);
                    let all_selected = keys.iter().all(|k| selected_keys.contains(k));
                    if all_selected {
                        for k in &keys {
                            selected_keys.remove(k);
                        }
                    } else {
                        selected_keys.extend(keys);
                    }
                }
                (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                    selected_keys.clear();
                }
                (KeyCode::Enter, _) => {
                    if selected_keys.is_empty() {
                        return Ok(None);
                    }
                    mode = Mode::Confirm;
                }
                _ => {}
            },
            Mode::Confirm => match (key.code, key.modifiers) {
                (KeyCode::Esc, _)
                | (KeyCode::Char('n'), _)
                | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    mode = Mode::List;
                }
                (KeyCode::Enter, _) | (KeyCode::Char('y'), _) => {
                    let mut action = build_action(&rows, &selected_keys);
                    action.confirmed = true;
                    return Ok(Some(action));
                }
                _ => {}
            },
        }

        // Defensive: if a render shrank the list to nothing selectable, keep the
        // highlight in-bounds.
        if nothing_selectable {
            highlighted = highlighted.min(rows.len().saturating_sub(1));
        }
    }
}

fn render_confirm<'a>(action: &CleanAction) -> Paragraph<'a> {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "Remove {} workspace(s) and delete {} branch(es)?",
            action.remove_worktrees.len(),
            action.delete_branches.len()
        ),
        Style::default().fg(Color::Red).bold(),
    ))];

    if action.remove_worktrees.iter().any(|w| w.is_current) {
        lines.push(Line::from(
            "The current workspace is selected; gx will switch you to the main workspace.",
        ));
    }
    lines.push(Line::from(""));

    for worktree in action.remove_worktrees.iter().take(10) {
        lines.push(Line::from(format!(
            "  workspace: {} ({})",
            worktree.name,
            worktree.path.display()
        )));
    }
    for branch in action.delete_branches.iter().take(10) {
        lines.push(Line::from(format!("  branch:    {}", branch)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Press enter/y to confirm, esc/n to go back."));

    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm Cleanup "),
        )
        .wrap(Wrap { trim: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn worktree(name: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            name: name.to_string(),
            path: PathBuf::from(format!("/ws/{}", name)),
            branch: branch.map(|b| b.to_string()),
            head: None,
            is_main: false,
            is_current: false,
            is_bare: false,
            is_locked: false,
        }
    }

    fn inputs() -> CleanInputs {
        let mut main = worktree("repo", Some("main"));
        main.is_main = true;
        let mut current = worktree("current-ws", Some("feature"));
        current.is_current = true;
        let mut locked = worktree("locked-ws", Some("locked-branch"));
        locked.is_locked = true;
        let removable = worktree("old-ws", Some("old-feature"));
        let protected_ws = worktree("staging-ws", Some("staging"));

        CleanInputs {
            worktrees: vec![main, current, locked, removable, protected_ws],
            orphan_branches: vec![
                ("orphan".to_string(), false, true),
                ("staging".to_string(), false, true),
            ],
            gone_branches: vec!["gone-branch".to_string()],
            protected: HashSet::from(["staging".to_string()]),
            ages: HashMap::new(),
        }
    }

    #[test]
    fn test_build_rows_sections_and_headers() {
        let rows = build_rows(&inputs());
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                CleanRow::Header(h) => Some(*h),
                _ => None,
            })
            .collect();
        assert_eq!(
            headers,
            vec![
                "Workspaces",
                "Local branches without workspaces",
                "Orphan branches whose remote tracking branch is gone",
            ]
        );
    }

    #[test]
    fn test_build_rows_disables_main_current_locked_protected() {
        let rows = build_rows(&inputs());

        let reason_for = |name: &str| -> Option<DisabledReason> {
            rows.iter().find_map(|r| match r {
                CleanRow::Workspace {
                    worktree, disabled, ..
                } if worktree.name == name => disabled.clone(),
                _ => None,
            })
        };

        assert_eq!(reason_for("repo"), Some(DisabledReason::Main));
        assert_eq!(reason_for("current-ws"), Some(DisabledReason::Current));
        assert_eq!(reason_for("locked-ws"), Some(DisabledReason::Locked));
        assert_eq!(reason_for("staging-ws"), Some(DisabledReason::Protected));
        assert_eq!(reason_for("old-ws"), None);
    }

    #[test]
    fn test_build_rows_disables_protected_orphan_branch() {
        let rows = build_rows(&inputs());
        let protected_orphan = rows.iter().find_map(|r| match r {
            CleanRow::OrphanBranch { name, disabled, .. } if name == "staging" => {
                Some(disabled.clone())
            }
            _ => None,
        });
        assert_eq!(protected_orphan, Some(Some(DisabledReason::Protected)));
    }

    #[test]
    fn test_selectable_keys_excludes_headers_and_disabled() {
        let rows = build_rows(&inputs());
        let keys = selectable_keys(&rows);

        // Only the removable workspace, the non-protected orphan, and the gone
        // branch are selectable.
        assert_eq!(keys.len(), 3);
        assert!(keys.iter().any(|k| k == "ws:/ws/old-ws"));
        assert!(keys.iter().any(|k| k == "branch:orphan"));
        assert!(keys.iter().any(|k| k == "branch:gone-branch"));
        assert!(!keys.iter().any(|k| k == "branch:staging"));
    }

    #[test]
    fn test_build_action_splits_workspaces_and_branches() {
        let rows = build_rows(&inputs());
        let selected = HashSet::from([
            "ws:/ws/old-ws".to_string(),
            "branch:orphan".to_string(),
            "branch:gone-branch".to_string(),
        ]);

        let action = build_action(&rows, &selected);
        assert_eq!(action.remove_worktrees.len(), 1);
        assert_eq!(action.remove_worktrees[0].name, "old-ws");
        let mut branches = action.delete_branches.clone();
        branches.sort();
        assert_eq!(
            branches,
            vec!["gone-branch".to_string(), "orphan".to_string()]
        );
    }

    #[test]
    fn test_build_action_dedupes_branch_listed_in_two_sections() {
        // A branch that appears as both an OrphanBranch and a GoneBranch shares
        // the "branch:<name>" key; build_action must not emit it twice (deleting
        // it twice would fail the second `git branch -d`).
        let rows = vec![
            CleanRow::OrphanBranch {
                name: "dup".to_string(),
                has_unpushed: false,
                has_upstream: false,
                disabled: None,
            },
            CleanRow::GoneBranch {
                name: "dup".to_string(),
                disabled: None,
            },
        ];
        let selected = HashSet::from(["branch:dup".to_string()]);

        let action = build_action(&rows, &selected);
        assert_eq!(action.delete_branches, vec!["dup".to_string()]);
    }

    #[test]
    fn test_build_rows_uses_precomputed_age() {
        let mut inputs = inputs();
        let removable_path = PathBuf::from("/ws/old-ws");
        inputs.ages = HashMap::from([(removable_path.clone(), 42)]);

        let rows = build_rows(&inputs);
        let age = rows.iter().find_map(|r| match r {
            CleanRow::Workspace {
                worktree, age_days, ..
            } if worktree.path == removable_path => Some(*age_days),
            _ => None,
        });
        assert_eq!(age, Some(Some(42)));

        // A workspace with no precomputed age falls back to None ("age?").
        let missing = rows.iter().find_map(|r| match r {
            CleanRow::Workspace {
                worktree, age_days, ..
            } if worktree.name == "current-ws" => Some(*age_days),
            _ => None,
        });
        assert_eq!(missing, Some(None));
    }

    #[test]
    fn test_next_selectable_skips_headers_and_disabled() {
        let rows = build_rows(&inputs());
        let first = first_selectable(&rows);
        // The first selectable row is the removable workspace, not a header or
        // the disabled main/current/locked rows that precede it.
        match &rows[first] {
            CleanRow::Workspace { worktree, .. } => assert_eq!(worktree.name, "old-ws"),
            other => panic!("expected workspace row, got {:?}", other),
        }

        // Moving up from the first selectable row stays put (nothing above it).
        assert_eq!(next_selectable(&rows, first, -1), first);

        // Moving down lands on the next selectable row, skipping the
        // intervening section header.
        let next = next_selectable(&rows, first, 1);
        assert!(next > first);
        assert!(rows[next].is_selectable());
    }
}
