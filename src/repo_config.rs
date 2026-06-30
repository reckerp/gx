//! Repo-level workspace policy: an optional, committable config that lives in
//! the repository (`.gx/workspace.toml`), an ignored local override
//! (`.gx/workspace.local.toml`), and a resolver that merges these with the
//! global confy config and the personal repo-setup profile into one
//! [`WorkspacePolicy`].
//!
//! This loader is intentionally separate from `confy` (which owns the global
//! user config) and from the hand-rolled parser in `repo_setup.rs` (which owns
//! the personal profile format). The shared repo config uses real `toml`
//! parsing via serde, with a `toml::Value` pre-pass for forward-compatible
//! version detection.

use crate::config;
use crate::repo_setup::{self, RepoSetupProfile};
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The highest `version` value this build understands. A higher declared
/// version triggers a warning (not a panic); serde ignores unknown fields, so
/// the known subset still loads.
pub const SUPPORTED_VERSION: i64 = 1;

/// The committable shared config.
pub const SHARED_FILE: &str = "workspace.toml";
/// The git-ignored local override.
pub const LOCAL_FILE: &str = "workspace.local.toml";

/// On-disk shape of `.gx/workspace.toml` / `.gx/workspace.local.toml`.
///
/// Every field is `Option`/defaulted so the merge can distinguish "unset"
/// (inherit the lower layer) from "set to empty".
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct RepoConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,

    #[serde(default)]
    pub workspace: RepoWorkspaceSection,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct RepoWorkspaceSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_files: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_script: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_strategy: Option<String>,

    // `clean` and `protection` belong to section 1 (the cleanup lifecycle
    // task). They are parsed and carried onto the resolved policy so the schema
    // is complete and that task can read them, but this task does not consume
    // them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean: Option<CleanSection>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HooksSection>,

    // See note on `clean` above: parsed for the cleanup task, unused here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protection: Option<ProtectionSection>,
}

/// Cleanup settings (consumed by section 1, not this task).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CleanSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_days: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct HooksSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_create: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_create: Option<Vec<String>>,
}

/// Branch protection list (consumed by section 1, not this task).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ProtectionSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branches: Option<Vec<String>>,
}

/// The single resolved policy that callers consume. Built by [`resolve`] from
/// all config layers; CLI flags are then applied on top by the caller.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspacePolicy {
    /// Repo-relative globs to copy into a new workspace. Unioned across layers
    /// (see [`resolve`]).
    pub copy_files: Vec<String>,

    /// A setup script to run after creation, if any.
    pub setup_script: Option<String>,

    /// The directory a relative `setup_script` is resolved against. Repo
    /// configs resolve against `main_root`; the personal profile resolves
    /// against its confy `repos/<key>` dir. Tracking the base disambiguates the
    /// two sources after merging.
    pub setup_script_base: Option<PathBuf>,

    pub update_strategy: Option<String>,

    pub pre_create_hooks: Vec<String>,
    pub post_create_hooks: Vec<String>,

    // Carried for section 1 (cleanup lifecycle); unused by this task.
    pub clean_threshold_days: Option<u64>,
    pub clean_auto: Option<bool>,
    pub protected_branches: Vec<String>,

    pub default_branch: Option<String>,
}

impl WorkspacePolicy {
    /// Absolute path to the configured setup script, resolved against
    /// [`Self::setup_script_base`]. Absolute scripts are returned as-is.
    pub fn resolved_setup_script(&self) -> Option<PathBuf> {
        let script = self.setup_script.as_deref()?;
        let p = Path::new(script);
        if p.is_absolute() {
            return Some(p.to_path_buf());
        }
        match &self.setup_script_base {
            Some(base) => Some(base.join(p)),
            None => Some(p.to_path_buf()),
        }
    }
}

/// Walk upward from `start` looking for a `.gx/` directory, stopping at the
/// filesystem root. Used so a shared config can be discovered from nested
/// directories within the repo.
pub fn discover_repo_dir(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(".gx");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

/// Load and parse a single repo config file. Returns `Ok(None)` when the file
/// does not exist. A declared `version` greater than [`SUPPORTED_VERSION`]
/// warns on stderr but still loads the known subset (serde ignores unknown
/// fields), satisfying the "unknown future version warns, not panics" rule.
pub fn load_file(path: &Path) -> Result<Option<RepoConfigFile>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path).into_diagnostic()?;

    // Forward-compatible version pre-pass: read `version` from a loose
    // `toml::Value` first so we can warn before strict deserialization.
    let value: toml::Value = toml::from_str(&content)
        .map_err(|e| miette!("failed to parse {}: {}", path.display(), e))?;
    if let Some(version) = value.get("version").and_then(toml::Value::as_integer)
        && version > SUPPORTED_VERSION
    {
        eprintln!(
            "warning: {} declares version {}; gx supports up to {}, ignoring unknown fields",
            path.display(),
            version,
            SUPPORTED_VERSION
        );
    }

    let parsed: RepoConfigFile = toml::from_str(&content)
        .map_err(|e| miette!("failed to parse {}: {}", path.display(), e))?;
    Ok(Some(parsed))
}

/// Locate `.gx/` under `main_root` and load the shared + local config files.
/// Returns `(shared, local)`, either of which may be `None` when absent.
pub fn load_repo_layers(
    main_root: &Path,
) -> Result<(Option<RepoConfigFile>, Option<RepoConfigFile>)> {
    // Anchor on the main worktree root, but use upward discovery so the config
    // is still found when `main_root` is itself nested (or a normalized path
    // differs slightly). Within a single repo this resolves to `main_root/.gx`.
    let gx_dir = discover_repo_dir(main_root).unwrap_or_else(|| main_root.join(".gx"));
    let shared = load_file(&gx_dir.join(SHARED_FILE))?;
    let local = load_file(&gx_dir.join(LOCAL_FILE))?;
    Ok((shared, local))
}

/// Merge all config layers into one resolved [`WorkspacePolicy`].
///
/// Precedence, lowest to highest:
/// 1. built-in defaults (the seed policy),
/// 2. global confy config (`config.workspace.copy_files`),
/// 3. the personal repo-setup profile (kept as an override source),
/// 4. shared `.gx/workspace.toml`,
/// 5. local `.gx/workspace.local.toml`.
///
/// CLI flags (the plan's layer 5) are applied by the caller after this returns.
///
/// Merge semantics differ by field:
/// - `copy_files` is **additive**: each layer's entries are appended onto the
///   accumulated list and deduped, so a repo *adds to* (does not silently erase)
///   personal/global copy files. This mirrors the existing
///   `repo_setup::run_setup_pipeline`, which unions global + personal copy
///   files.
/// - everything else is **replace**: a higher layer with `Some(_)` overrides;
///   `None` inherits the lower layer.
pub fn resolve(
    global: &config::Config,
    personal: &RepoSetupProfile,
    shared: Option<&RepoConfigFile>,
    local: Option<&RepoConfigFile>,
    main_root: &Path,
) -> WorkspacePolicy {
    let mut policy = WorkspacePolicy::default();

    // Layer 2: global confy config.
    let mut copy_files: Vec<String> = global.workspace.copy_files.clone();

    // Layer 3: personal repo-setup profile (override source per the plan).
    copy_files.extend(personal.config.copy_files.iter().cloned());
    if let Some(script) = &personal.config.setup_script {
        policy.setup_script = Some(script.clone());
        // The personal profile resolves its relative script against its confy
        // `repos/<key>` dir (existing behavior).
        policy.setup_script_base = Some(personal.dir.clone());
    }

    // Layers 4 and 5: shared then local repo config.
    for layer in [shared, local].into_iter().flatten() {
        if let Some(default_branch) = &layer.default_branch {
            policy.default_branch = Some(default_branch.clone());
        }

        let ws = &layer.workspace;

        // copy_files: additive union (see doc comment).
        if let Some(files) = &ws.copy_files {
            copy_files.extend(files.iter().cloned());
        }

        // setup_script: scalar replace. Repo scripts resolve against
        // main_root (the plan example uses ".gx/setup-workspace.sh").
        if let Some(script) = &ws.setup_script {
            policy.setup_script = Some(script.clone());
            policy.setup_script_base = Some(main_root.to_path_buf());
        }

        if let Some(strategy) = &ws.update_strategy {
            policy.update_strategy = Some(strategy.clone());
        }

        if let Some(hooks) = &ws.hooks {
            if let Some(pre) = &hooks.pre_create {
                policy.pre_create_hooks = pre.clone();
            }
            if let Some(post) = &hooks.post_create {
                policy.post_create_hooks = post.clone();
            }
        }

        // clean / protection: parsed and carried for section 1 (unused here).
        if let Some(clean) = &ws.clean {
            if let Some(days) = clean.threshold_days {
                policy.clean_threshold_days = Some(days);
            }
            if let Some(auto) = clean.auto {
                policy.clean_auto = Some(auto);
            }
        }
        if let Some(protection) = &ws.protection
            && let Some(branches) = &protection.branches
        {
            policy.protected_branches = branches.clone();
        }
    }

    dedupe(&mut copy_files);
    policy.copy_files = copy_files;
    policy
}

/// Convenience: load the layers under `main_root` and resolve them together
/// with the global config and personal profile. Returns the resolved policy.
pub fn resolve_for_repo(main_root: &Path) -> Result<WorkspacePolicy> {
    let global = config::load()?;
    let personal = repo_setup::profile_for_repo(main_root)?;
    let (shared, local) = load_repo_layers(main_root)?;
    Ok(resolve(
        &global,
        &personal,
        shared.as_ref(),
        local.as_ref(),
        main_root,
    ))
}

/// Variables available to hook templates.
#[derive(Debug, Clone)]
pub struct HookVars {
    /// Workspace directory name.
    pub workspace: String,
    /// Absolute path to the new workspace.
    pub workspace_path: PathBuf,
    /// Absolute path to the main worktree.
    pub main_root: PathBuf,
    /// Checked-out branch.
    pub branch: String,
}

/// Expand `{workspace}`, `{workspace_path}`, `{main_root}` and `{branch}` in a
/// hook template. Unknown `{...}` tokens are left as-is. Multi-pass literal
/// `String::replace`, matching the codebase's `{repo}` substitution style.
pub fn expand_hook(template: &str, vars: &HookVars) -> String {
    template
        .replace(
            "{workspace_path}",
            &vars.workspace_path.display().to_string(),
        )
        .replace("{workspace}", &vars.workspace)
        .replace("{main_root}", &vars.main_root.display().to_string())
        .replace("{branch}", &vars.branch)
}

/// Run a list of hooks via `sh -c`, with `cwd` as the working directory and the
/// hook variables also exported as `GX_*` environment variables. Each hook's
/// stdout is redirected to stderr so gx's own stdout stays clean (it carries
/// the cd target the shell wrapper consumes).
///
/// When `abort_on_failure` (pre-create), a non-zero exit returns `Err` so the
/// caller can abort creation. Otherwise (post-create) a failure warns on stderr
/// and continues.
pub fn run_hooks(
    hooks: &[String],
    vars: &HookVars,
    cwd: &Path,
    abort_on_failure: bool,
) -> Result<()> {
    for hook in hooks {
        let expanded = expand_hook(hook, vars);
        if expanded.trim().is_empty() {
            continue;
        }

        let phase = if abort_on_failure {
            "pre-create"
        } else {
            "post-create"
        };
        eprintln!("Running {} hook: {}", phase, expanded);

        let stdout = repo_setup::stderr_stdio()?;
        let status = Command::new("sh")
            .arg("-c")
            .arg(&expanded)
            .current_dir(cwd)
            .env("GX_WORKSPACE", &vars.workspace)
            .env("GX_WORKSPACE_PATH", &vars.workspace_path)
            .env("GX_MAIN_ROOT", &vars.main_root)
            .env("GX_BRANCH", &vars.branch)
            .stdin(Stdio::inherit())
            .stdout(stdout)
            .stderr(Stdio::inherit())
            .status()
            .into_diagnostic()?;

        if !status.success() {
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string());
            if abort_on_failure {
                return Err(miette!(
                    "pre-create hook failed (status {}): {}",
                    code,
                    expanded
                ));
            } else {
                eprintln!(
                    "warning: post-create hook failed (status {}); keeping workspace: {}",
                    code, expanded
                );
            }
        }
    }

    Ok(())
}

fn dedupe(values: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

/// Contents written to `.gx/.gitignore`: the local override and transient state
/// must never be committed.
pub const GITIGNORE_CONTENTS: &str = "workspace.local.toml\nstate.toml\ntmp/\n";

/// Return `main_root/.gx`, creating the directory if it does not exist.
pub fn ensure_gx_dir(main_root: &Path) -> Result<PathBuf> {
    let dir = main_root.join(".gx");
    std::fs::create_dir_all(&dir).into_diagnostic()?;
    Ok(dir)
}

/// Write a [`RepoConfigFile`] to `path` as TOML.
pub fn write_config_file(path: &Path, config: &RepoConfigFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).into_diagnostic()?;
    }
    let toml = toml::to_string_pretty(config)
        .map_err(|e| miette!("failed to serialize {}: {}", path.display(), e))?;
    std::fs::write(path, toml).into_diagnostic()?;
    Ok(())
}

/// Write `.gx/.gitignore` if it does not already exist. Returns whether it was
/// created.
pub fn ensure_gitignore(gx_dir: &Path) -> Result<bool> {
    let path = gx_dir.join(".gitignore");
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(&path, GITIGNORE_CONTENTS).into_diagnostic()?;
    Ok(true)
}

/// Create an empty (commented) local override if one does not already exist.
/// Returns whether it was created.
pub fn ensure_local_override(gx_dir: &Path) -> Result<bool> {
    let path = gx_dir.join(LOCAL_FILE);
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(
        &path,
        "# Local, machine-specific overrides for .gx/workspace.toml.\n\
         # This file is git-ignored and never committed.\n\n\
         [workspace]\n",
    )
    .into_diagnostic()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_setup::{RepoSetupConfig, RepoSetupProfile};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "gx-repo-config-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn empty_personal(dir: &Path) -> RepoSetupProfile {
        RepoSetupProfile {
            dir: dir.to_path_buf(),
            config_path: dir.join("config.toml"),
            config: RepoSetupConfig::default(),
        }
    }

    #[test]
    fn test_discovered_from_nested_directories() {
        let root = temp_dir("nested");
        std::fs::create_dir_all(root.join(".gx")).unwrap();
        let nested = root.join("apps").join("web").join("src");
        std::fs::create_dir_all(&nested).unwrap();

        let found = discover_repo_dir(&nested).expect("should discover .gx from nested dir");
        assert_eq!(found, root.join(".gx"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_discover_returns_none_without_gx_dir() {
        let root = temp_dir("no-gx");
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        // No .gx anywhere under root, but parents of root may have one; anchor
        // the search by checking only that our temp subtree has none.
        let found = discover_repo_dir(&nested);
        // The search may walk above our temp dir; only assert it does not find
        // one *inside* our isolated subtree.
        if let Some(found) = found {
            assert!(!found.starts_with(&root));
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_local_override_wins_over_shared() {
        // update_strategy is a scalar -> higher layer (local) replaces shared.
        let main_root = temp_dir("override");
        let shared = RepoConfigFile {
            version: Some(1),
            default_branch: None,
            workspace: RepoWorkspaceSection {
                update_strategy: Some("rebase".to_string()),
                ..Default::default()
            },
        };
        let local = RepoConfigFile {
            version: Some(1),
            default_branch: None,
            workspace: RepoWorkspaceSection {
                update_strategy: Some("merge".to_string()),
                ..Default::default()
            },
        };
        let global = config::Config::default();
        let personal = empty_personal(&main_root);

        let policy = resolve(&global, &personal, Some(&shared), Some(&local), &main_root);
        assert_eq!(policy.update_strategy.as_deref(), Some("merge"));

        std::fs::remove_dir_all(&main_root).ok();
    }

    #[test]
    fn test_local_setup_script_overrides_shared() {
        let main_root = temp_dir("script-override");
        let shared = RepoConfigFile {
            workspace: RepoWorkspaceSection {
                setup_script: Some(".gx/shared.sh".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let local = RepoConfigFile {
            workspace: RepoWorkspaceSection {
                setup_script: Some(".gx/local.sh".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let global = config::Config::default();
        let personal = empty_personal(&main_root);

        let policy = resolve(&global, &personal, Some(&shared), Some(&local), &main_root);
        assert_eq!(policy.setup_script.as_deref(), Some(".gx/local.sh"));
        assert_eq!(
            policy.resolved_setup_script(),
            Some(main_root.join(".gx/local.sh"))
        );

        std::fs::remove_dir_all(&main_root).ok();
    }

    #[test]
    fn test_global_config_works_when_no_repo_config() {
        let main_root = temp_dir("global-only");
        let mut global = config::Config::default();
        global.workspace.copy_files = vec![".env".to_string(), ".env.local".to_string()];
        let personal = empty_personal(&main_root);

        let policy = resolve(&global, &personal, None, None, &main_root);
        assert_eq!(
            policy.copy_files,
            vec![".env".to_string(), ".env.local".to_string()]
        );
        assert!(policy.setup_script.is_none());
        assert!(policy.pre_create_hooks.is_empty());

        std::fs::remove_dir_all(&main_root).ok();
    }

    #[test]
    fn test_copy_files_union_across_layers() {
        let main_root = temp_dir("copy-union");
        let mut global = config::Config::default();
        global.workspace.copy_files = vec![".env".to_string()];

        let mut personal = empty_personal(&main_root);
        personal.config.copy_files = vec!["personal.toml".to_string()];

        let shared = RepoConfigFile {
            workspace: RepoWorkspaceSection {
                // includes a duplicate of the global entry to exercise dedupe
                copy_files: Some(vec![".env".to_string(), "shared.toml".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };
        let local = RepoConfigFile {
            workspace: RepoWorkspaceSection {
                copy_files: Some(vec!["local.toml".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };

        let policy = resolve(&global, &personal, Some(&shared), Some(&local), &main_root);
        assert_eq!(
            policy.copy_files,
            vec![
                ".env".to_string(),
                "personal.toml".to_string(),
                "shared.toml".to_string(),
                "local.toml".to_string(),
            ]
        );

        std::fs::remove_dir_all(&main_root).ok();
    }

    #[test]
    fn test_unknown_future_version_warns_not_panics() {
        let root = temp_dir("future-version");
        let gx = root.join(".gx");
        std::fs::create_dir_all(&gx).unwrap();
        std::fs::write(
            gx.join(SHARED_FILE),
            "version = 999\n\n[workspace]\ncopy_files = [\".env\"]\nfuture_field = \"x\"\n",
        )
        .unwrap();

        // Must not panic; deserialization ignores unknown fields.
        let loaded = load_file(&gx.join(SHARED_FILE)).expect("should not error");
        let file = loaded.expect("file exists");
        assert_eq!(file.version, Some(999));
        assert_eq!(file.workspace.copy_files, Some(vec![".env".to_string()]));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_load_file_missing_returns_none() {
        let root = temp_dir("missing");
        let result = load_file(&root.join(".gx").join(SHARED_FILE)).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_load_repo_layers_parses_full_example() {
        let main_root = temp_dir("full-example");
        let gx = main_root.join(".gx");
        std::fs::create_dir_all(&gx).unwrap();
        std::fs::write(
            gx.join(SHARED_FILE),
            r#"version = 1
default_branch = "main"

[workspace]
copy_files = [".env.example", ".env.local"]
setup_script = ".gx/setup-workspace.sh"
update_strategy = "rebase"

[workspace.clean]
threshold_days = 10
auto = false

[workspace.hooks]
pre_create = ["test -f package.json"]
post_create = ["pnpm install"]

[workspace.protection]
branches = ["staging", "release"]
"#,
        )
        .unwrap();

        let (shared, local) = load_repo_layers(&main_root).unwrap();
        assert!(local.is_none());
        let shared = shared.expect("shared config present");
        assert_eq!(shared.version, Some(1));
        assert_eq!(shared.default_branch.as_deref(), Some("main"));

        let global = config::Config::default();
        let personal = empty_personal(&main_root);
        let policy = resolve(&global, &personal, Some(&shared), None, &main_root);

        assert_eq!(
            policy.pre_create_hooks,
            vec!["test -f package.json".to_string()]
        );
        assert_eq!(policy.post_create_hooks, vec!["pnpm install".to_string()]);
        assert_eq!(policy.update_strategy.as_deref(), Some("rebase"));
        assert_eq!(policy.clean_threshold_days, Some(10));
        assert_eq!(policy.clean_auto, Some(false));
        assert_eq!(
            policy.protected_branches,
            vec!["staging".to_string(), "release".to_string()]
        );
        assert_eq!(policy.default_branch.as_deref(), Some("main"));
        assert_eq!(
            policy.resolved_setup_script(),
            Some(main_root.join(".gx/setup-workspace.sh"))
        );

        std::fs::remove_dir_all(&main_root).ok();
    }

    #[test]
    fn test_expand_hook_replaces_all_variables() {
        let vars = HookVars {
            workspace: "feat-x".to_string(),
            workspace_path: PathBuf::from("/tmp/ws/feat-x"),
            main_root: PathBuf::from("/tmp/main"),
            branch: "feat/x".to_string(),
        };

        assert_eq!(expand_hook("{workspace}", &vars), "feat-x");
        assert_eq!(expand_hook("{workspace_path}", &vars), "/tmp/ws/feat-x");
        assert_eq!(expand_hook("{main_root}", &vars), "/tmp/main");
        assert_eq!(expand_hook("{branch}", &vars), "feat/x");

        // {workspace} must not clobber {workspace_path}.
        assert_eq!(
            expand_hook("cp {main_root}/.env {workspace_path}/.env", &vars),
            "cp /tmp/main/.env /tmp/ws/feat-x/.env"
        );

        // Combined and unknown tokens.
        assert_eq!(
            expand_hook("echo {workspace} {branch} {unknown}", &vars),
            "echo feat-x feat/x {unknown}"
        );
    }

    #[test]
    fn test_run_hooks_pre_create_aborts_on_failure() {
        let cwd = temp_dir("pre-abort");
        let vars = HookVars {
            workspace: "w".to_string(),
            workspace_path: cwd.clone(),
            main_root: cwd.clone(),
            branch: "b".to_string(),
        };
        let result = run_hooks(&["false".to_string()], &vars, &cwd, true);
        assert!(result.is_err(), "pre-create hook failure must abort");

        // A succeeding pre-create hook returns Ok.
        let ok = run_hooks(&["true".to_string()], &vars, &cwd, true);
        assert!(ok.is_ok());

        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn test_run_hooks_post_create_failure_does_not_abort() {
        let cwd = temp_dir("post-warn");
        let vars = HookVars {
            workspace: "w".to_string(),
            workspace_path: cwd.clone(),
            main_root: cwd.clone(),
            branch: "b".to_string(),
        };
        // Post-create failure only warns; the call still succeeds, so the
        // caller keeps the workspace.
        let result = run_hooks(&["false".to_string()], &vars, &cwd, false);
        assert!(result.is_ok(), "post-create hook failure must not abort");

        std::fs::remove_dir_all(&cwd).ok();
    }
}
