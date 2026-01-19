use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub aliases: HashMap<String, String>,

    #[serde(default)]
    pub ai: AiConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_model() -> String {
    "opencode/big-pickle".to_string()
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
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
    let config: Config =
        confy::load("gx", None).map_err(|e| miette::miette!("Failed to load config: {}", e))?;
    Ok(config)
}

pub fn load_path() -> miette::Result<std::path::PathBuf> {
    let path = confy::get_configuration_file_path("gx", None)
        .map_err(|e| miette::miette!("Failed to get config path: {}", e))?;
    Ok(path)
}
