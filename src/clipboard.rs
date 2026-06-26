use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Copy text to the system clipboard by piping it to the platform's clipboard
/// tool. On Linux several tools are tried in turn (Wayland first, then X11) so
/// it works across desktop setups without pulling in a clipboard dependency.
///
/// Returns an error if no clipboard tool could be spawned, mirroring the
/// dependency-free, shell-out approach used by [`crate::browser`].
pub fn copy(text: &str) -> io::Result<()> {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };

    let mut last_err = io::Error::new(
        io::ErrorKind::NotFound,
        "no clipboard tool available on PATH",
    );
    for (program, args) in candidates {
        match try_copy(program, args, text) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

fn try_copy(program: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    // Drop stdin after writing so the tool sees EOF and exits.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} exited unsuccessfully")))
    }
}
