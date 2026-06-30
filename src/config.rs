use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub bind: String,
    pub public_base_url: String,
    pub state_file: String,
    pub admin: AdminConfig,
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdminConfig {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub api_server: Option<String>,
    #[serde(deserialize_with = "deserialize_send_to")]
    pub send_to: Vec<i64>,
}

fn deserialize_send_to<'de, D>(deserializer: D) -> Result<Vec<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value = toml::Value::deserialize(deserializer)?;
    match value {
        toml::Value::Integer(n) => Ok(vec![n]),
        toml::Value::Array(arr) => arr
            .iter()
            .map(|v| match v {
                toml::Value::Integer(n) => Ok(*n),
                other => Err(D::Error::custom(format!("expected integer, got {other}"))),
            })
            .collect(),
        other => Err(D::Error::custom(format!(
            "send_to must be integer or array, got {other}"
        ))),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiConfig {
    #[serde(default = "default_true")]
    pub require_auth: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { require_auth: true }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepoConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    pub name: String,
    pub git_url: String,
    pub local_path: String,
    pub webhook_secret: String,
    #[serde(default = "default_glob")]
    pub scripts_glob: String,
    #[serde(default = "default_branch")]
    pub branch: String,
}

impl RepoConfig {
    /// Stable identity key used to look up this repo's persistent UUID in the
    /// state file. Prefers `local_path`; falls back to `git_url` when empty.
    pub fn identity(&self) -> &str {
        if !self.local_path.is_empty() {
            &self.local_path
        } else {
            &self.git_url
        }
    }

    pub fn set_uuid(&mut self, uuid: String) {
        self.uuid = Some(uuid);
    }
}

fn default_glob() -> String {
    "**/*.user.js".to_string()
}

fn default_branch() -> String {
    "master".to_string()
}

pub fn load_config(path: &Path) -> anyhow::Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&content).context("failed to parse config.toml")
}
