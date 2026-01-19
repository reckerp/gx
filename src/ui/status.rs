use super::{status_char, status_color};
use crate::git::status::{FileStatus, RepoStatus};
use ratatui::prelude::*;

pub fn render_status(status: &RepoStatus) {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(render_branch_line(status));
    lines.push(Line::raw(""));

    if let Some(ref msg) = status.last_commit_message {
        lines.push(render_commit_line(msg, status.last_commit_time.as_deref()));
        lines.push(Line::raw(""));
    }

    if !status.staged_files.is_empty() {
        lines.push(render_section_header(
            "Staged",
            status.staged_files.len(),
            Color::Green,
        ));
        for file in &status.staged_files {
            lines.push(render_file_line(file.status, &file.path, true));
        }
        lines.push(Line::raw(""));
    }

    if !status.unstaged_files.is_empty() {
        lines.push(render_section_header(
            "Changes",
            status.unstaged_files.len(),
            Color::Yellow,
        ));
        for file in &status.unstaged_files {
            lines.push(render_file_line(file.status, &file.path, false));
        }
        lines.push(Line::raw(""));
    }

    if status.staged_files.is_empty() && status.unstaged_files.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("✓ ", Style::default().fg(Color::Green)),
            Span::styled("Working tree clean", Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::raw(""));
    }

    if status.stash_count > 0 {
        lines.push(render_stash_line(status.stash_count));
        lines.push(Line::raw(""));
    }

    lines.push(render_hints(status));

    for line in lines {
        print_line(&line);
    }
}

fn render_branch_line(status: &RepoStatus) -> Line<'static> {
    let mut spans = Vec::new();

    if status.branch.is_detached {
        spans.push(Span::styled("◎ ", Style::default().fg(Color::Yellow)));
        spans.push(Span::styled(
            status.branch.name.clone(),
            Style::default().fg(Color::Yellow).bold(),
        ));
        spans.push(Span::styled(
            " (detached)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::styled("⎇ ", Style::default().fg(Color::Cyan)));
        spans.push(Span::styled(
            status.branch.name.clone(),
            Style::default().fg(Color::Cyan).bold(),
        ));
    }

    if let Some(ref remote) = status.remote {
        spans.push(Span::styled(" ← ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            remote.remote.clone(),
            Style::default().fg(Color::DarkGray),
        ));

        if remote.ahead > 0 || remote.behind > 0 {
            spans.push(Span::raw(" "));
            if remote.ahead > 0 {
                spans.push(Span::styled(
                    format!("↑{}", remote.ahead),
                    Style::default().fg(Color::Green),
                ));
            }
            if remote.behind > 0 {
                if remote.ahead > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(
                    format!("↓{}", remote.behind),
                    Style::default().fg(Color::Red),
                ));
            }
        }
    }

    Line::from(spans)
}

fn render_commit_line(message: &str, time: Option<&str>) -> Line<'static> {
    let mut spans = vec![
        Span::styled("● ", Style::default().fg(Color::Magenta)),
        Span::styled(truncate(message, 50), Style::default().fg(Color::White)),
    ];

    if let Some(t) = time {
        spans.push(Span::styled(
            format!(" ({})", t),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}

fn render_section_header(title: &str, count: usize, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{} ", title), Style::default().fg(color).bold()),
        Span::styled(format!("({})", count), Style::default().fg(Color::DarkGray)),
    ])
}

fn render_file_line(status: FileStatus, path: &str, _staged: bool) -> Line<'static> {
    let icon = status_char(status);
    let color = status_color(status);

    Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(path.to_string(), Style::default().fg(Color::White)),
    ])
}

fn render_stash_line(count: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("⚑ ", Style::default().fg(Color::Blue)),
        Span::styled(
            format!("{} stash{}", count, if count == 1 { "" } else { "es" }),
            Style::default().fg(Color::Blue),
        ),
    ])
}

fn render_hints(status: &RepoStatus) -> Line<'static> {
    let mut hints = Vec::new();

    if !status.unstaged_files.is_empty() {
        hints.push(Span::styled("gx add", Style::default().fg(Color::Yellow)));
        hints.push(Span::styled(" stage", Style::default().fg(Color::DarkGray)));
    }

    if !status.staged_files.is_empty() {
        if !hints.is_empty() {
            hints.push(Span::styled("  ", Style::default()));
        }
        hints.push(Span::styled(
            "gx commit",
            Style::default().fg(Color::Yellow),
        ));
        hints.push(Span::styled(
            " commit",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if let Some(ref remote) = status.remote
        && remote.ahead > 0
    {
        if !hints.is_empty() {
            hints.push(Span::styled("  ", Style::default()));
        }
        hints.push(Span::styled("gx push", Style::default().fg(Color::Yellow)));
        hints.push(Span::styled(" push", Style::default().fg(Color::DarkGray)));
    }

    if status.stash_count > 0 {
        if !hints.is_empty() {
            hints.push(Span::styled("  ", Style::default()));
        }
        hints.push(Span::styled("gxsp", Style::default().fg(Color::Yellow)));
        hints.push(Span::styled(
            " pop stash",
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(hints)
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}

fn print_line(line: &Line) {
    use crossterm::style::Stylize;
    use std::io::{self, Write};

    let mut stdout = io::stdout();

    for span in &line.spans {
        let mut styled = span.content.to_string().stylize();

        if let Some(fg) = span.style.fg {
            styled = apply_color(styled, fg);
        }

        if span.style.add_modifier.contains(Modifier::BOLD) {
            styled = styled.bold();
        }

        let _ = write!(stdout, "{}", styled);
    }
    println!();
}

fn apply_color(
    styled: crossterm::style::StyledContent<String>,
    color: Color,
) -> crossterm::style::StyledContent<String> {
    use crossterm::style::Stylize;

    match color {
        Color::Black => styled.black(),
        Color::Red | Color::LightRed => styled.red(),
        Color::Green | Color::LightGreen => styled.green(),
        Color::Yellow | Color::LightYellow => styled.yellow(),
        Color::Blue | Color::LightBlue => styled.blue(),
        Color::Magenta | Color::LightMagenta => styled.magenta(),
        Color::Cyan | Color::LightCyan => styled.cyan(),
        Color::Gray => styled.grey(),
        Color::DarkGray => styled.dark_grey(),
        Color::White => styled.white(),
        _ => styled,
    }
}
