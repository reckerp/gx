use crate::config;
use crate::git;
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoSetupConfig {
    #[serde(default)]
    pub copy_files: Vec<String>,

    #[serde(default)]
    pub setup_script: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RepoSetupProfile {
    pub dir: PathBuf,
    pub config_path: PathBuf,
    pub config: RepoSetupConfig,
}

#[derive(Debug)]
pub struct SetupReport {
    pub copied: Vec<String>,
    pub script: ScriptRun,
}

#[derive(Debug)]
pub enum ScriptRun {
    Skipped,
    Succeeded(PathBuf),
    Failed { path: PathBuf, status: ExitStatus },
}

#[derive(Debug, Clone)]
pub struct CopyCandidate {
    pub path: String,
    pub is_dir: bool,
}

pub fn profile_for_repo(main_root: &Path) -> Result<RepoSetupProfile> {
    let config_dir = gx_config_dir()?;
    let common_git_dir = git::worktree::common_git_dir().map_err(|e| miette!("{}", e))?;
    let key = repo_key(main_root, &common_git_dir);
    let dir = config_dir.join("repos").join(key);
    let config_path = dir.join("config.toml");
    let config = load_repo_config(&config_path)?;

    Ok(RepoSetupProfile {
        dir,
        config_path,
        config,
    })
}

pub fn save_profile(profile: &RepoSetupProfile) -> Result<()> {
    if let Some(parent) = profile.config_path.parent() {
        std::fs::create_dir_all(parent).into_diagnostic()?;
    }

    let mut file = std::fs::File::create(&profile.config_path).into_diagnostic()?;
    writeln!(file, "copy_files = [").into_diagnostic()?;
    for path in &profile.config.copy_files {
        writeln!(file, "  \"{}\",", escape_toml_string(path)).into_diagnostic()?;
    }
    writeln!(file, "]").into_diagnostic()?;

    if let Some(script) = &profile.config.setup_script {
        writeln!(file).into_diagnostic()?;
        writeln!(file, "setup_script = \"{}\"", escape_toml_string(script)).into_diagnostic()?;
    }

    Ok(())
}

pub fn setup_script_path(profile: &RepoSetupProfile) -> Option<PathBuf> {
    profile
        .config
        .setup_script
        .as_deref()
        .map(|script| profile.dir.join(script))
}

pub fn default_setup_script_path(profile: &RepoSetupProfile) -> PathBuf {
    profile.dir.join("setup.sh")
}

pub fn create_default_setup_script(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).into_diagnostic()?;
    }

    if !path.exists() {
        std::fs::write(
            path,
            "#!/usr/bin/env bash\nset -euo pipefail\n\n# This script runs from the new gx workspace root.\n",
        )
        .into_diagnostic()?;
    }

    make_executable(path)?;
    Ok(())
}

pub fn discover_copy_candidates(root: &Path) -> Result<Vec<CopyCandidate>> {
    let mut candidates = Vec::new();
    collect_candidates(root, Path::new(""), &mut candidates)?;
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(candidates)
}

pub fn run_setup_pipeline(
    main_root: &Path,
    workspace_root: &Path,
    global_copy_files: &[String],
    run_script: bool,
) -> Result<SetupReport> {
    let profile = profile_for_repo(main_root)?;
    let mut patterns = global_copy_files.to_vec();
    patterns.extend(profile.config.copy_files.iter().cloned());
    dedupe(&mut patterns);

    let copied = copy_setup_files(main_root, workspace_root, &patterns)?;
    let script = if run_script {
        run_setup_script(workspace_root, main_root, &profile)?
    } else {
        ScriptRun::Skipped
    };

    Ok(SetupReport { copied, script })
}

/// Copy configured setup files from `src_root` to `dst_root`.
/// Patterns are repository-relative globs. `*` and `?` match within one path
/// component; `**` matches zero or more components. Directories are copied
/// recursively. Missing sources are skipped.
pub fn copy_setup_files(
    src_root: &Path,
    dst_root: &Path,
    patterns: &[String],
) -> Result<Vec<String>> {
    let mut copied = Vec::new();
    let mut copied_set = HashSet::new();

    for pattern in patterns {
        let pattern = normalize_pattern(pattern)?;
        if pattern.is_empty() {
            continue;
        }

        let matches = matched_paths(src_root, &pattern)?;
        for rel_path in matches {
            if !copied_set.insert(rel_path.clone()) {
                continue;
            }

            let src = src_root.join(&rel_path);
            let dst = dst_root.join(&rel_path);

            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).into_diagnostic()?;
            }

            if src.is_dir() {
                copy_dir_recursive(&src, &dst)?;
            } else {
                std::fs::copy(&src, &dst).into_diagnostic()?;
            }

            copied.push(rel_path);
        }
    }

    Ok(copied)
}

pub fn open_in_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string());

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{} \"$1\"", editor))
        .arg("gx-editor")
        .arg(path)
        .status()
        .into_diagnostic()?;

    if !status.success() {
        return Err(miette!(
            "editor '{}' exited with status {}",
            editor,
            display_status(status)
        ));
    }

    Ok(())
}

fn gx_config_dir() -> Result<PathBuf> {
    let path = config::load_path()?;
    path.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| miette!("gx config path has no parent: {}", path.display()))
}

fn repo_key(main_root: &Path, common_git_dir: &Path) -> String {
    let repo_name = main_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let repo_name = sanitize_path_component(repo_name);
    let hash = stable_hash(&common_git_dir.display().to_string());
    format!("{}-{:016x}", repo_name, hash)
}

fn sanitize_path_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }

    if out.is_empty() {
        "repo".to_string()
    } else {
        out
    }
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn load_repo_config(path: &Path) -> Result<RepoSetupConfig> {
    if !path.exists() {
        return Ok(RepoSetupConfig::default());
    }

    let content = std::fs::read_to_string(path).into_diagnostic()?;
    parse_repo_config(path, &content)
}

fn parse_repo_config(path: &Path, content: &str) -> Result<RepoSetupConfig> {
    let mut config = RepoSetupConfig::default();
    let mut lines = content.lines().enumerate().peekable();

    while let Some((line_idx, line)) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("copy_files") {
            let Some(rest) = rest.trim_start().strip_prefix('=') else {
                return Err(parse_error(path, line_idx, "expected '=' after copy_files"));
            };
            let mut array = rest.trim().to_string();
            while !array_terminated(&array) {
                let Some((_, next)) = lines.next() else {
                    return Err(parse_error(path, line_idx, "unterminated copy_files array"));
                };
                array.push('\n');
                array.push_str(next);
            }
            config.copy_files = extract_quoted_strings(&array);
        } else if let Some(rest) = trimmed.strip_prefix("setup_script") {
            let Some(rest) = rest.trim_start().strip_prefix('=') else {
                return Err(parse_error(
                    path,
                    line_idx,
                    "expected '=' after setup_script",
                ));
            };
            config.setup_script = extract_quoted_strings(rest).into_iter().next();
        } else {
            return Err(parse_error(path, line_idx, "unknown repo setup config key"));
        }
    }

    Ok(config)
}

fn parse_error(path: &Path, line_idx: usize, message: &str) -> miette::Report {
    miette!(
        "failed to parse {} at line {}: {}",
        path.display(),
        line_idx + 1,
        message
    )
}

/// True once `s` contains a `]` that closes the array, i.e. one that sits
/// outside any quoted string. A bare `contains(']')` would stop accumulating at
/// a `]` embedded in a quoted path entry, silently dropping later entries.
fn array_terminated(s: &str) -> bool {
    let mut in_quote = false;
    let mut escaped = false;
    for ch in s.chars() {
        if in_quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_quote = false;
            }
        } else if ch == '"' {
            in_quote = true;
        } else if ch == ']' {
            return true;
        }
    }
    false
}

fn extract_quoted_strings(input: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }

        let mut value = String::new();
        let mut escaped = false;
        for ch in chars.by_ref() {
            if escaped {
                match ch {
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    other => value.push(other),
                }
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                break;
            } else {
                value.push(ch);
            }
        }
        strings.push(value);
    }

    strings
}

/// Escape a value for a TOML basic string. Newlines, tabs and carriage returns
/// must be escaped (an unescaped newline ends the string for the reader and
/// corrupts the round-trip); the decode side in `extract_quoted_strings` mirrors
/// these escapes exactly.
fn escape_toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

fn dedupe(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn normalize_pattern(pattern: &str) -> Result<String> {
    let pattern = pattern.trim().trim_matches('/').to_string();
    if pattern.is_empty() {
        return Ok(pattern);
    }

    let path = Path::new(&pattern);
    if path.is_absolute() || pattern.contains('\\') {
        return Err(miette!(
            "invalid setup copy pattern '{}': patterns must be repo-relative paths",
            pattern
        ));
    }

    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(miette!(
            "invalid setup copy pattern '{}': parent or absolute components are not allowed",
            pattern
        ));
    }

    Ok(pattern)
}

fn matched_paths(root: &Path, pattern: &str) -> Result<Vec<String>> {
    if !has_glob(pattern) {
        let path = root.join(pattern);
        return Ok(if path.exists() && !is_git_meta_path(pattern) {
            vec![pattern.to_string()]
        } else {
            vec![]
        });
    }

    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let mut paths = Vec::new();
    collect_matching_paths(root, Path::new(""), &pattern_parts, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_matching_paths(
    root: &Path,
    rel_dir: &Path,
    pattern_parts: &[&str],
    paths: &mut Vec<String>,
) -> Result<()> {
    let dir = root.join(rel_dir);
    if !dir.is_dir() {
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .into_diagnostic()?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }

        let rel_path = rel_dir.join(&name);
        let rel_string = path_to_slash_string(&rel_path);
        let rel_parts: Vec<&str> = rel_string.split('/').collect();

        if glob_parts_match(pattern_parts, &rel_parts) {
            paths.push(rel_string);
        }

        if entry.path().is_dir() {
            collect_matching_paths(root, &rel_path, pattern_parts, paths)?;
        }
    }

    Ok(())
}

fn collect_candidates(
    root: &Path,
    rel_dir: &Path,
    candidates: &mut Vec<CopyCandidate>,
) -> Result<()> {
    let dir = root.join(rel_dir);
    if !dir.is_dir() {
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .into_diagnostic()?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }

        let rel_path = rel_dir.join(&name);
        let rel_string = path_to_slash_string(&rel_path);
        let is_dir = entry.path().is_dir();
        candidates.push(CopyCandidate {
            path: rel_string,
            is_dir,
        });

        if is_dir {
            collect_candidates(root, &rel_path, candidates)?;
        }
    }

    Ok(())
}

fn glob_parts_match(pattern: &[&str], text: &[&str]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(&"**"), _) => {
            glob_parts_match(&pattern[1..], text)
                || (!text.is_empty() && glob_parts_match(pattern, &text[1..]))
        }
        (Some(part), Some(text_part)) if wildcard_match(part, text_part) => {
            glob_parts_match(&pattern[1..], &text[1..])
        }
        _ => false,
    }
}

fn has_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

fn is_git_meta_path(path: &str) -> bool {
    path.split('/').any(|part| part == ".git")
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).into_diagnostic()?;

    let mut entries: Vec<_> = std::fs::read_dir(src)
        .into_diagnostic()?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if entry.file_name().to_string_lossy() == ".git" {
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).into_diagnostic()?;
        }
    }

    Ok(())
}

/// Simple wildcard matcher supporting `*` (any sequence) and `?` (any char)
/// within one path component.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    fn matches(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => matches(&p[1..], t) || (!t.is_empty() && matches(p, &t[1..])),
            (Some('?'), Some(_)) => matches(&p[1..], &t[1..]),
            (Some(pc), Some(tc)) if pc == tc => matches(&p[1..], &t[1..]),
            _ => false,
        }
    }

    matches(&p, &t)
}

fn run_setup_script(
    workspace_root: &Path,
    main_root: &Path,
    profile: &RepoSetupProfile,
) -> Result<ScriptRun> {
    let Some(script_path) = setup_script_path(profile) else {
        return Ok(ScriptRun::Skipped);
    };

    if !script_path.exists() {
        return Ok(ScriptRun::Skipped);
    }

    let stdout = stderr_stdio()?;
    let status = Command::new(&script_path)
        .current_dir(workspace_root)
        .env("GX_WORKSPACE_ROOT", workspace_root)
        .env("GX_MAIN_ROOT", main_root)
        .env("GX_REPO_CONFIG_DIR", &profile.dir)
        .stdin(Stdio::inherit())
        .stdout(stdout)
        .stderr(Stdio::inherit())
        .status()
        .into_diagnostic()?;

    if status.success() {
        Ok(ScriptRun::Succeeded(script_path))
    } else {
        Ok(ScriptRun::Failed {
            path: script_path,
            status,
        })
    }
}

/// Stdio that sends the setup script's stdout to gx's stderr, keeping gx's own
/// stdout clean (it carries the cd target path the shell wrapper consumes).
/// Prefers `/dev/stderr`, but falls back to a duplicate of the real stderr fd so
/// a missing/unwritable `/dev/stderr` doesn't abort an otherwise-runnable script.
fn stderr_stdio() -> Result<Stdio> {
    if let Ok(file) = OpenOptions::new().write(true).open("/dev/stderr") {
        return Ok(Stdio::from(file));
    }

    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        if let Ok(owned) = std::io::stderr().as_fd().try_clone_to_owned() {
            return Ok(Stdio::from(owned));
        }
    }

    Ok(Stdio::inherit())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).into_diagnostic()?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).into_diagnostic()?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

fn path_to_slash_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn display_status(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_match() {
        assert!(wildcard_match(".env", ".env"));
        assert!(wildcard_match(".env*", ".env"));
        assert!(wildcard_match(".env*", ".env.local"));
        assert!(wildcard_match("*.local", ".env.local"));
        assert!(wildcard_match("?env", ".env"));
        assert!(!wildcard_match(".env", ".env.local"));
        assert!(!wildcard_match(".env?", ".env"));
        assert!(!wildcard_match("*.toml", ".env"));
    }

    #[test]
    fn test_glob_parts_match_supports_recursive_glob() {
        assert!(glob_parts_match(&["**", ".env.local"], &[".env.local"]));
        assert!(glob_parts_match(
            &["**", ".env.local"],
            &["apps", "web", ".env.local"]
        ));
        assert!(glob_parts_match(
            &["*", ".env.local"],
            &["apps", ".env.local"]
        ));
        assert!(!glob_parts_match(
            &["*", ".env.local"],
            &["apps", "web", ".env.local"]
        ));
    }

    #[test]
    fn test_copy_setup_files_never_copies_git() {
        let tmp =
            std::env::temp_dir().join(format!("gx-repo-setup-git-test-{}", std::process::id()));
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        std::fs::write(src.join(".gitignore"), "target").unwrap();

        let patterns = vec![".*".to_string(), ".git".to_string()];
        let copied = copy_setup_files(&src, &dst, &patterns).unwrap();

        assert_eq!(copied, vec![".gitignore".to_string()]);
        assert!(!dst.join(".git").exists());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_copy_setup_files_with_recursive_globs() {
        let tmp = std::env::temp_dir().join(format!("gx-repo-setup-test-{}", std::process::id()));
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(src.join("apps/web")).unwrap();
        std::fs::create_dir_all(src.join(".vercel")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        std::fs::write(src.join(".env"), "SECRET=1").unwrap();
        std::fs::write(src.join(".env.local"), "LOCAL=1").unwrap();
        std::fs::write(src.join("apps/web/.env.local"), "APP_LOCAL=1").unwrap();
        std::fs::write(src.join(".vercel/project.json"), "{}").unwrap();

        let patterns = vec![
            "**/.env.local".to_string(),
            ".env".to_string(),
            ".vercel/".to_string(),
            "missing.txt".to_string(),
        ];
        let copied = copy_setup_files(&src, &dst, &patterns).unwrap();

        assert_eq!(
            copied,
            vec![
                ".env.local".to_string(),
                "apps/web/.env.local".to_string(),
                ".env".to_string(),
                ".vercel".to_string()
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dst.join(".env")).unwrap(),
            "SECRET=1"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("apps/web/.env.local")).unwrap(),
            "APP_LOCAL=1"
        );
        assert!(dst.join(".vercel/project.json").exists());
        assert!(!dst.join("missing.txt").exists());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_array_terminated_ignores_bracket_inside_quotes() {
        assert!(!array_terminated("[\"foo]bar\","));
        assert!(array_terminated("[\"foo]bar\"]"));
        assert!(array_terminated("[\"a\", \"b\"]"));
        assert!(!array_terminated("[\"a\","));
    }

    #[test]
    fn test_parse_repo_config_keeps_entries_after_bracket_in_value() {
        // A path entry containing ']' must not prematurely end the array.
        let content = "copy_files = [\n  \"weird]name\",\n  \".env\",\n]\n";
        let config = parse_repo_config(Path::new("config.toml"), content).unwrap();
        assert_eq!(
            config.copy_files,
            vec!["weird]name".to_string(), ".env".to_string()]
        );
    }

    #[test]
    fn test_escape_toml_string_round_trips_control_chars() {
        for value in [
            "plain",
            "with\nnewline",
            "tab\tand\rcr",
            "quote\" and \\ backslash",
            "bracket]inside",
        ] {
            let escaped = format!("\"{}\"", escape_toml_string(value));
            let decoded = extract_quoted_strings(&escaped);
            assert_eq!(
                decoded,
                vec![value.to_string()],
                "round-trip failed for {value:?}"
            );
        }
    }
}
