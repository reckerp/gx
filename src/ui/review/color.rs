//! Terminal color-depth handling for the diff view.
//!
//! syntect themes and the diff palette are authored in 24-bit RGB. Many
//! terminals — Apple's Terminal.app, and tmux/screen unless explicitly
//! configured for `RGB`/`Tc` — silently *drop* 24-bit (`38;2;…`) escapes, which
//! shows up as a diff with no syntax colors and no add/remove backgrounds (the
//! "missing syntax highlighting" symptom). To stay legible everywhere, we
//! downsample every RGB color to the 256-color palette unless we are confident
//! the terminal handles truecolor.

use ratatui::style::Color;

/// The color resolution the diff view will actually emit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    /// 24-bit RGB, emitted verbatim.
    TrueColor,
    /// The xterm 256-color palette; RGB colors are mapped to the nearest index.
    Ansi256,
}

/// Resolve the configured preference into a concrete depth.
///
/// `"always"`/`"truecolor"`/`"24bit"` force truecolor; `"never"`/`"256"` force
/// 256-color; anything else auto-detects.
pub fn detect(pref: &str) -> ColorDepth {
    match pref {
        "always" | "truecolor" | "24bit" => ColorDepth::TrueColor,
        "never" | "256" | "ansi256" => ColorDepth::Ansi256,
        _ => detect_auto(&Env::from_process()),
    }
}

/// The environment inputs auto-detection reads (extracted so it is testable).
struct Env {
    term: String,
    colorterm: String,
    term_program: String,
    in_multiplexer: bool,
}

impl Env {
    fn from_process() -> Self {
        let get = |k: &str| std::env::var(k).unwrap_or_default();
        Env {
            term: get("TERM"),
            colorterm: get("COLORTERM"),
            term_program: get("TERM_PROGRAM"),
            in_multiplexer: std::env::var_os("TMUX").is_some()
                || std::env::var_os("STY").is_some(),
        }
    }
}

fn detect_auto(env: &Env) -> ColorDepth {
    // Terminals that advertise direct color in TERM (e.g. `xterm-direct`) always
    // mean it.
    if env.term.contains("direct") {
        return ColorDepth::TrueColor;
    }

    let advertises = matches!(env.colorterm.as_str(), "truecolor" | "24bit");

    // Inside tmux/screen, COLORTERM is usually inherited from the outer terminal
    // but the multiplexer only forwards 24-bit when specially configured — which
    // we cannot detect at runtime. Be conservative: fall back to 256-color
    // (which every terminal renders) and let `truecolor = "always"` opt back in.
    let in_mux = env.in_multiplexer
        || env.term.starts_with("screen")
        || env.term.starts_with("tmux")
        || matches!(env.term_program.as_str(), "tmux" | "screen");

    if advertises && !in_mux {
        ColorDepth::TrueColor
    } else {
        ColorDepth::Ansi256
    }
}

/// Adapt a color to the target depth: RGB colors are mapped to the nearest
/// 256-color index under [`ColorDepth::Ansi256`]; everything else (named ANSI
/// colors, `Reset`, already-indexed) passes through unchanged.
pub fn adapt(color: Color, depth: ColorDepth) -> Color {
    match (depth, color) {
        (ColorDepth::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(rgb_to_ansi256(r, g, b)),
        _ => color,
    }
}

/// The six RGB levels of the xterm color cube.
const CUBE_STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// Map one channel to its nearest cube index (0..=5). Matches the thresholds
/// tmux and other terminals use.
fn channel_to_cube(v: u8) -> usize {
    if v < 48 {
        0
    } else if v < 114 {
        1
    } else {
        ((v as usize - 35) / 40).min(5)
    }
}

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| {
        let d = x as i32 - y as i32;
        (d * d) as u32
    };
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// Nearest xterm 256-color index for an RGB triple, choosing between the 6×6×6
/// color cube (16..=231) and the grayscale ramp (232..=255) by which is closer.
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let (qr, qg, qb) = (
        channel_to_cube(r),
        channel_to_cube(g),
        channel_to_cube(b),
    );
    let cube = (CUBE_STEPS[qr], CUBE_STEPS[qg], CUBE_STEPS[qb]);
    let cube_idx = 16 + 36 * qr + 6 * qg + qb;

    // Nearest point on the 24-step grayscale ramp (levels 8, 18, … 238).
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gray_i = if avg <= 8 {
        0
    } else {
        (((avg - 8) + 5) / 10).min(23)
    } as usize;
    let gray_level = (8 + 10 * gray_i) as u8;
    let gray = (gray_level, gray_level, gray_level);
    let gray_idx = 232 + gray_i;

    let target = (r, g, b);
    if dist(gray, target) < dist(cube, target) {
        gray_idx as u8
    } else {
        cube_idx as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(term: &str, colorterm: &str, term_program: &str, mux: bool) -> Env {
        Env {
            term: term.into(),
            colorterm: colorterm.into(),
            term_program: term_program.into(),
            in_multiplexer: mux,
        }
    }

    #[test]
    fn tmux_with_inherited_truecolor_falls_back_to_256() {
        // The reported case: COLORTERM=truecolor but running under tmux, which
        // drops 24-bit unless configured. Must not claim truecolor.
        let e = env("tmux-256color", "truecolor", "tmux", true);
        assert_eq!(detect_auto(&e), ColorDepth::Ansi256);
    }

    #[test]
    fn plain_iterm_truecolor_is_truecolor() {
        let e = env("xterm-256color", "truecolor", "iTerm.app", false);
        assert_eq!(detect_auto(&e), ColorDepth::TrueColor);
    }

    #[test]
    fn terminal_app_without_colorterm_is_256() {
        // macOS Terminal.app: 256-color capable, no 24-bit, no COLORTERM.
        let e = env("xterm-256color", "", "Apple_Terminal", false);
        assert_eq!(detect_auto(&e), ColorDepth::Ansi256);
    }

    #[test]
    fn direct_term_is_truecolor_even_in_mux() {
        let e = env("xterm-direct", "", "", true);
        assert_eq!(detect_auto(&e), ColorDepth::TrueColor);
    }

    #[test]
    fn explicit_preference_overrides_detection() {
        assert_eq!(detect("always"), ColorDepth::TrueColor);
        assert_eq!(detect("never"), ColorDepth::Ansi256);
    }

    #[test]
    fn adapt_only_touches_rgb_under_256() {
        assert_eq!(
            adapt(Color::Rgb(255, 0, 0), ColorDepth::Ansi256),
            Color::Indexed(196)
        );
        // Truecolor passes RGB through.
        assert_eq!(
            adapt(Color::Rgb(255, 0, 0), ColorDepth::TrueColor),
            Color::Rgb(255, 0, 0)
        );
        // Named / reset colors are never rewritten.
        assert_eq!(adapt(Color::Cyan, ColorDepth::Ansi256), Color::Cyan);
        assert_eq!(adapt(Color::Reset, ColorDepth::Ansi256), Color::Reset);
    }

    #[test]
    fn rgb_to_ansi256_maps_known_anchors() {
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16); // cube black
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196); // pure red
        assert_eq!(rgb_to_ansi256(0, 255, 0), 46); // pure green
        assert_eq!(rgb_to_ansi256(0, 0, 255), 21); // pure blue
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231); // cube white
    }

    #[test]
    fn rgb_to_ansi256_prefers_grayscale_for_neutral_tones() {
        // A mid gray should land on the grayscale ramp (232..=255), not the cube.
        let idx = rgb_to_ansi256(128, 128, 128);
        assert!((232..=255).contains(&idx), "got {idx}");
    }
}
