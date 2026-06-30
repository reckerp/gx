use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    OpenCode,
    Claude,
}

impl Agent {
    pub fn as_str(&self) -> &str {
        match self {
            Agent::OpenCode => "opencode",
            Agent::Claude => "claude",
        }
    }
}

impl std::fmt::Display for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl TryFrom<String> for Agent {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "opencode" => Ok(Agent::OpenCode),
            "claude" => Ok(Agent::Claude),
            _ => Err(format!(
                "Unknown agent: {}. Expected 'opencode' or 'claude'",
                value
            )),
        }
    }
}

impl<'a> TryFrom<&'a str> for Agent {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "opencode" => Ok(Agent::OpenCode),
            "claude" => Ok(Agent::Claude),
            _ => Err(format!(
                "Unknown agent: {}. Expected 'opencode' or 'claude'",
                value
            )),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub aliases: HashMap<String, String>,

    #[serde(default)]
    pub ai: AiConfig,

    #[serde(default)]
    pub workspace: WorkspaceConfig,

    #[serde(default)]
    pub pr: PrConfig,

    #[serde(default)]
    pub review: ReviewConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PrConfig {
    /// Orgs to search in the dashboard's org scope (`--owner` per entry). Empty
    /// means the org scope is omitted from the scope cycle.
    #[serde(default)]
    pub orgs: Vec<String>,

    /// Default merge method for the merge action: "squash", "merge", or "rebase".
    #[serde(default = "default_merge_method")]
    pub merge_method: String,

    /// Whether reviewer suggestion falls back to the AI agent when the
    /// deterministic signal is thin.
    #[serde(default = "default_reviewer_ai_fallback")]
    pub reviewer_ai_fallback: bool,
}

fn default_merge_method() -> String {
    "squash".to_string()
}

fn default_reviewer_ai_fallback() -> bool {
    true
}

impl Default for PrConfig {
    fn default() -> Self {
        PrConfig {
            orgs: Vec::new(),
            merge_method: default_merge_method(),
            reviewer_ai_fallback: default_reviewer_ai_fallback(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// syntect theme name for diff syntax highlighting (e.g. "base16-ocean.dark").
    #[serde(default = "default_review_theme")]
    pub theme: String,

    /// Minimum terminal width (columns) for side-by-side; below this the diff
    /// falls back to a unified single-column view.
    #[serde(default = "default_side_by_side_min_width")]
    pub side_by_side_min_width: u16,

    /// Default range mode when none is given on the CLI: "branch", "commit",
    /// or "uncommitted".
    #[serde(default = "default_review_mode")]
    pub default_mode: String,
}

fn default_review_theme() -> String {
    "base16-ocean.dark".to_string()
}

fn default_side_by_side_min_width() -> u16 {
    120
}

fn default_review_mode() -> String {
    "branch".to_string()
}

impl Default for ReviewConfig {
    fn default() -> Self {
        ReviewConfig {
            theme: default_review_theme(),
            side_by_side_min_width: default_side_by_side_min_width(),
            default_mode: default_review_mode(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Where workspaces are created. `{repo}` is replaced by the repository
    /// directory name. Supports `~` for the home directory and absolute
    /// paths; relative paths are resolved against the main worktree root.
    #[serde(default = "default_workspace_root")]
    pub root: String,

    /// Files copied from the main worktree into new workspaces (e.g. ".env").
    /// Paths are relative to the repo root; `*`/`?` match within one path
    /// component and `**` matches zero or more components. Missing files are
    /// skipped silently.
    #[serde(default = "default_copy_files")]
    pub copy_files: Vec<String>,

    /// Branches that cleanup must never remove, on top of the always-protected
    /// set (default branch, `main`, `master`, the current branch, and any
    /// branch checked out in an active worktree). Maps to `[workspace]
    /// protected_branches` in the config TOML.
    #[serde(default)]
    pub protected_branches: Vec<String>,

    /// Cleanup-lifecycle settings (`[workspace.clean]`).
    #[serde(default)]
    pub clean: WorkspaceCleanConfig,
}

fn default_workspace_root() -> String {
    "~/gx/workspaces/{repo}".to_string()
}

fn default_copy_files() -> Vec<String> {
    vec![".env".to_string()]
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        WorkspaceConfig {
            root: default_workspace_root(),
            copy_files: default_copy_files(),
            protected_branches: Vec::new(),
            clean: WorkspaceCleanConfig::default(),
        }
    }
}

/// Cleanup-lifecycle settings, mapped to the `[workspace.clean]` table.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceCleanConfig {
    /// A workspace is "stale" once its conservative age reaches this many days.
    /// Only consulted by `gx workspace clean --auto --use-threshold`.
    #[serde(default = "default_threshold_days")]
    pub threshold_days: u64,

    /// When true, bare `gx workspace clean` behaves as `--auto`.
    #[serde(default)]
    pub auto: bool,
}

fn default_threshold_days() -> u64 {
    7
}

impl Default for WorkspaceCleanConfig {
    fn default() -> Self {
        WorkspaceCleanConfig {
            threshold_days: default_threshold_days(),
            auto: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default = "default_agent")]
    pub agent: String,

    #[serde(default = "default_model")]
    pub model: String,
}

fn default_agent() -> String {
    "opencode".to_string()
}

fn default_model() -> String {
    "opencode/big-pickle".to_string()
}

impl AiConfig {
    pub fn get_agent(&self) -> Result<Agent, String> {
        self.agent.as_str().try_into()
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            agent: default_agent(),
            model: default_model(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut aliases = HashMap::new();

        aliases.insert("gch".to_string(), "checkout".to_string());
        aliases.insert("gco".to_string(), "checkout".to_string());
        aliases.insert("gs".to_string(), "status".to_string());
        aliases.insert("ga".to_string(), "add".to_string());
        aliases.insert("gc".to_string(), "commit".to_string());
        aliases.insert("gp".to_string(), "push".to_string());
        aliases.insert("gst".to_string(), "stash".to_string());
        aliases.insert("gl".to_string(), "log".to_string());
        aliases.insert("gws".to_string(), "workspace".to_string());
        aliases.insert("gpr".to_string(), "pr".to_string());

        Config {
            aliases,
            ai: AiConfig::default(),
            workspace: WorkspaceConfig::default(),
            pr: PrConfig::default(),
            review: ReviewConfig::default(),
        }
    }
}

pub fn load() -> miette::Result<Config> {
    let config: Config = confy::load("gx", Some("config"))
        .map_err(|e| miette::miette!("Failed to load config: {}", e))?;
    Ok(config)
}

/// Persist `config` back to the user's global gx config file. Used by
/// `gx workspace protect`/`unprotect` to update the protected-branch list.
pub fn store(config: &Config) -> miette::Result<()> {
    confy::store("gx", Some("config"), config)
        .map_err(|e| miette::miette!("Failed to save config: {}", e))?;
    Ok(())
}

pub fn load_path() -> miette::Result<std::path::PathBuf> {
    let path = confy::get_configuration_file_path("gx", Some("config"))
        .map_err(|e| miette::miette!("Failed to get config path: {}", e))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_review_config() {
        let review = ReviewConfig::default();
        assert_eq!(review.theme, "base16-ocean.dark");
        assert_eq!(review.side_by_side_min_width, 120);
        assert_eq!(review.default_mode, "branch");
        assert_eq!(Config::default().review.theme, "base16-ocean.dark");
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.ai.agent, "opencode");
        assert_eq!(config.ai.model, "opencode/big-pickle");
        assert_eq!(config.workspace.root, "~/gx/workspaces/{repo}");
        assert_eq!(config.workspace.copy_files, vec![".env".to_string()]);
        assert!(config.workspace.protected_branches.is_empty());
        assert_eq!(config.workspace.clean.threshold_days, 7);
        assert!(!config.workspace.clean.auto);
        assert_eq!(config.aliases.get("gpr").map(String::as_str), Some("pr"));
        assert_eq!(config.pr.merge_method, "squash");
        assert!(config.pr.reviewer_ai_fallback);
        assert!(config.pr.orgs.is_empty());
    }

    #[test]
    fn test_default_workspace_clean_config() {
        let clean = WorkspaceCleanConfig::default();
        assert_eq!(clean.threshold_days, 7);
        assert!(!clean.auto);

        let workspace = WorkspaceConfig::default();
        assert!(workspace.protected_branches.is_empty());
        assert_eq!(workspace.clean.threshold_days, 7);
    }

    #[test]
    fn test_workspace_config_roundtrips_clean_and_protected() {
        // Serialize a config with the new fields and read it back, exercising
        // both the Serialize and Deserialize derives end-to-end.
        let mut config = Config::default();
        config.workspace.protected_branches = vec!["staging".to_string(), "release".to_string()];
        config.workspace.clean.threshold_days = 10;
        config.workspace.clean.auto = true;

        let json = serde_json::to_string(&config).expect("config should serialize");
        let restored: Config = serde_json::from_str(&json).expect("config should deserialize");

        assert_eq!(
            restored.workspace.protected_branches,
            vec!["staging".to_string(), "release".to_string()]
        );
        assert_eq!(restored.workspace.clean.threshold_days, 10);
        assert!(restored.workspace.clean.auto);
    }

    #[test]
    fn test_workspace_config_without_clean_keeps_defaults() {
        // Older config files omit the new tables entirely; serde defaults must
        // keep them loading and populate sensible values.
        let restored: Config =
            serde_json::from_str(r#"{"workspace":{"root":".worktrees"}}"#).expect("parses");
        assert_eq!(restored.workspace.root, ".worktrees");
        assert_eq!(restored.workspace.clean.threshold_days, 7);
        assert!(!restored.workspace.clean.auto);
        assert!(restored.workspace.protected_branches.is_empty());
    }

    #[test]
    fn test_default_pr_config() {
        let pr = PrConfig::default();
        assert_eq!(pr.merge_method, "squash");
        assert!(pr.reviewer_ai_fallback);
        assert!(pr.orgs.is_empty());
    }

    #[test]
    fn test_default_ai_config() {
        let ai_config = AiConfig::default();
        assert_eq!(ai_config.agent, "opencode");
        assert_eq!(ai_config.model, "opencode/big-pickle");
    }

    #[test]
    fn test_agent_from_str() {
        assert!(matches!(Agent::try_from("opencode"), Ok(Agent::OpenCode)));
        assert!(matches!(Agent::try_from("claude"), Ok(Agent::Claude)));
    }

    #[test]
    fn test_agent_from_string() {
        assert!(matches!(
            Agent::try_from("opencode".to_string()),
            Ok(Agent::OpenCode)
        ));
        assert!(matches!(
            Agent::try_from("claude".to_string()),
            Ok(Agent::Claude)
        ));
    }

    #[test]
    fn test_agent_invalid() {
        assert!(Agent::try_from("invalid").is_err());
        assert!(Agent::try_from("invalid".to_string()).is_err());
    }

    #[test]
    fn test_agent_as_str() {
        assert_eq!(Agent::OpenCode.as_str(), "opencode");
        assert_eq!(Agent::Claude.as_str(), "claude");
    }

    #[test]
    fn test_ai_config_get_agent() {
        let config = AiConfig::default();
        assert!(matches!(config.get_agent(), Ok(Agent::OpenCode)));
    }

    #[test]
    fn test_ai_config_get_agent_invalid() {
        let config = AiConfig {
            agent: "invalid".to_string(),
            model: "test".to_string(),
        };
        assert!(config.get_agent().is_err());
    }
}
