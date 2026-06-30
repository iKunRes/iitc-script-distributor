use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{RwLock, RwLockReadGuard};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct State {
    pub repos: HashMap<String, RepoState>,
    /// Persistent UUID assignment keyed by a repo's stable identity
    /// (its `local_path`, falling back to `git_url`). This is the source of
    /// truth for repo UUIDs so they survive `config.toml` being rewritten.
    #[serde(default)]
    pub repo_uuids: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoState {
    pub scripts: HashMap<String, ScriptEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptEntry {
    pub relative_path: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub url_slug: String,
    pub url_override_update: Option<String>,
    pub url_override_download: Option<String>,
    #[serde(default)]
    pub missing: bool,
    #[serde(default)]
    pub disabled: bool,
}

pub struct SharedState {
    inner: RwLock<State>,
    state_file: String,
}

impl SharedState {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let state = if std::path::Path::new(path).exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read state file {path}"))?;
            serde_json::from_str(&content).context("failed to parse state.json")?
        } else {
            State::default()
        };
        Ok(Self {
            inner: RwLock::new(state),
            state_file: path.to_string(),
        })
    }

    pub async fn read(&self) -> RwLockReadGuard<'_, State> {
        self.inner.read().await
    }

    /// Return the persistent UUID for the given stable identity key, minting
    /// and persisting a fresh one if none exists yet. UUIDs live in the state
    /// file rather than `config.toml` so they are stable across config edits.
    pub async fn resolve_repo_uuid(&self, identity: &str) -> anyhow::Result<String> {
        {
            let guard = self.inner.read().await;
            if let Some(uuid) = guard.repo_uuids.get(identity) {
                return Ok(uuid.clone());
            }
        }
        let new_uuid = uuid::Uuid::new_v4().to_string();
        self.write_and_save(|state| {
            state
                .repo_uuids
                .entry(identity.to_string())
                .or_insert_with(|| new_uuid.clone());
        })
        .await?;
        let guard = self.inner.read().await;
        Ok(guard
            .repo_uuids
            .get(identity)
            .cloned()
            .expect("uuid was just inserted"))
    }

    pub async fn write_and_save(&self, f: impl FnOnce(&mut State)) -> anyhow::Result<()> {
        let mut guard = self.inner.write().await;
        f(&mut guard);
        let json = serde_json::to_string_pretty(&*guard).context("failed to serialize state")?;
        std::fs::write(&self.state_file, json)
            .with_context(|| format!("failed to write state file {}", self.state_file))?;
        Ok(())
    }
}
