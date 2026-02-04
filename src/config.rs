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

        Config {
            aliases,
            ai: AiConfig::default(),
        }
    }
}

pub fn load() -> miette::Result<Config> {
    let config: Config = confy::load("gx", Some("config"))
        .map_err(|e| miette::miette!("Failed to load config: {}", e))?;
    Ok(config)
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
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.ai.agent, "opencode");
        assert_eq!(config.ai.model, "opencode/big-pickle");
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
