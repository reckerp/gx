//! `gx review` orchestration: resolve the diff range, enumerate the changed
//! files, and launch the review TUI.

use crate::config;
use crate::git::review::{diff, range};
use crate::ui;
use miette::Result;

pub fn run(target: Option<String>, base: Option<String>) -> Result<()> {
    let cfg = config::load()?;
    // Honor [review] default_mode when no explicit target/base is given.
    let range = if target.is_none() && base.is_none() && cfg.review.default_mode == "uncommitted" {
        range::resolve_uncommitted()?
    } else {
        range::resolve(target, base)?
    };
    let files = diff::changed_files(&range)?;

    // Detect the terminal appearance before the TUI takes over the terminal, and
    // pick a matching syntect theme when none is configured.
    let appearance = ui::review::detect_appearance(&cfg.review.appearance);
    let theme = if cfg.review.theme.is_empty() {
        match appearance {
            ui::review::Appearance::Light => "InspiredGitHub",
            ui::review::Appearance::Dark => "base16-ocean.dark",
        }
        .to_string()
    } else {
        cfg.review.theme.clone()
    };

    // Resolve the terminal color depth: themes are 24-bit RGB, but many
    // terminals/multiplexers drop truecolor escapes (which reads as "no syntax
    // highlighting"), so we downsample to 256-color unless truecolor is safe.
    let color_depth = ui::review::color::detect(&cfg.review.truecolor);

    ui::review::run(
        range,
        files,
        &theme,
        cfg.review.side_by_side_min_width,
        appearance,
        color_depth,
        cfg.review.tab_width,
    )
}
