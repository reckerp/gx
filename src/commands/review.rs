//! `gx review` orchestration: resolve the diff range, enumerate the changed
//! files, and launch the review TUI.

use crate::config;
use crate::git::review::{diff, range};
use crate::ui;
use miette::Result;

pub fn run(target: Option<String>, base: Option<String>) -> Result<()> {
    let range = range::resolve(target, base)?;
    let files = diff::changed_files(&range)?;
    let cfg = config::load()?;

    ui::review::run(
        range,
        files,
        &cfg.review.theme,
        cfg.review.side_by_side_min_width,
    )
}
