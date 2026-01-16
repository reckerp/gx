use crate::git::git_exec::{self, ExecOptions};
use miette::Result;

pub fn run(args: Vec<String>) -> Result<()> {
    git_exec::exec(args, ExecOptions::default())?;
    Ok(())
}
