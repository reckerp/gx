use crate::git::git_exec::{self, ExecOptions};
use miette::Result;

pub fn run(mut args: Vec<String>) -> Result<()> {
    // Support the explicit `gx git <command>` form without running `git git <command>`
    if args.first().map(String::as_str) == Some("git") {
        args.remove(0);
    }

    git_exec::exec(args, ExecOptions::default())?;
    Ok(())
}
