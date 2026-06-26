use std::process::{Command, Stdio};

/// Open a URL in the user's default browser.
///
/// Spawns the platform's "open" helper detached, with all stdio silenced so it
/// can never write into the TUI's alternate screen, and returns without waiting
/// for the browser to exit. Returns an error only if the helper itself could
/// not be spawned (e.g. it is missing from PATH).
pub fn open(url: &str) -> std::io::Result<()> {
    let (program, leading_args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(target_os = "windows") {
        // `start` is a cmd builtin; the empty "" is the window title so a URL
        // with spaces is not mistaken for one.
        ("cmd", &["/C", "start", ""])
    } else {
        ("xdg-open", &[])
    };

    Command::new(program)
        .args(leading_args)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}
