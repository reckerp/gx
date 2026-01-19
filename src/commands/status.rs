use crate::git::status;
use crate::ui::status::render_status;
use miette::Result;

pub fn run() -> Result<()> {
    let status = status::get_repo_status()?;
    render_status(&status);
    Ok(())
}
