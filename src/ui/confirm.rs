use crossterm::cursor::MoveToColumn;
use crossterm::event::{self, Event, KeyCode};
use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use miette::IntoDiagnostic;
use std::io::{self, Write};

// The prompt is rendered with plain ANSI on the given writer instead of a
// ratatui inline viewport: inline viewports read the cursor position, and
// crossterm writes that query to stdout. When stdout is captured (e.g. by
// the 'gx setup' shell wrapper doing cd "$(gx ws ...)") the query never
// reaches the terminal and the read times out with "The cursor position
// could not be read within a normal duration".

pub fn run(message: &str) -> miette::Result<bool> {
    let mut stdout = io::stdout();
    run_inner(message, &mut stdout)
}

/// Confirm prompt rendered on stderr, keeping stdout clean for
/// machine-readable output (e.g. paths consumed by the shell wrapper).
pub fn run_on_stderr(message: &str) -> miette::Result<bool> {
    let mut stderr = io::stderr();
    run_inner(message, &mut stderr)
}

fn run_inner<W: Write>(message: &str, writer: &mut W) -> miette::Result<bool> {
    enable_raw_mode().into_diagnostic()?;
    let result = prompt(message, writer);
    disable_raw_mode().ok();
    writeln!(writer).ok();
    result
}

fn prompt<W: Write>(message: &str, writer: &mut W) -> miette::Result<bool> {
    let mut selected = 0; // 0 = Yes, 1 = No

    loop {
        render(message, selected, writer).into_diagnostic()?;

        if let Event::Key(key) = event::read().into_diagnostic()? {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') => selected = 0,
                KeyCode::Right | KeyCode::Char('l') => selected = 1,
                KeyCode::Char('y' | 'Y') => return Ok(true),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => return Ok(false),
                KeyCode::Enter => return Ok(selected == 0),
                _ => {}
            }
        }
    }
}

fn render<W: Write>(message: &str, selected: usize, writer: &mut W) -> io::Result<()> {
    queue!(
        writer,
        MoveToColumn(0),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Yellow),
        Print(message),
        Print("  "),
        ResetColor,
    )?;

    if selected == 0 {
        queue!(
            writer,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Green)
        )?;
    } else {
        queue!(writer, SetForegroundColor(Color::Green))?;
    }
    queue!(writer, Print(" y "), ResetColor, Print(" "))?;

    if selected == 1 {
        queue!(
            writer,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Red)
        )?;
    } else {
        queue!(writer, SetForegroundColor(Color::Red))?;
    }
    queue!(
        writer,
        Print(" n "),
        ResetColor,
        SetForegroundColor(Color::DarkGrey),
        Print("  (y/n)"),
        ResetColor
    )?;

    writer.flush()
}
