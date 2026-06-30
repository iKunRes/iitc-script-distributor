use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;
use serde::Deserialize;
use std::sync::atomic::Ordering;
use toml;

use crate::AppState;
use crate::admin::templates::render;

#[derive(Deserialize)]
pub struct FlashQuery {
    #[serde(default)]
    pub flash: String,
}

#[derive(Debug)]
struct ScriptView {
    uuid: String,
    name: String,
    version: String,
    description: String,
    url_slug: String,
    effective_update_url: String,
    effective_download_url: String,
    has_override: bool,
    missing: bool,
}

#[derive(Debug)]
struct RepoView {
    name: String,
    uuid: String,
    scripts: Vec<ScriptView>,
}

pub async fn list(State(app): State<AppState>, Query(q): Query<FlashQuery>) -> Response {
    let state = app.state.read().await;
    let base = app.config.public_base_url.trim_end_matches('/');

    let mut repos: Vec<RepoView> = Vec::new();
    for repo_cfg in &app.config.repos {
        let repo_uuid = match &repo_cfg.uuid {
            Some(u) => u.clone(),
            None => continue,
        };
        let repo_state = state.repos.get(&repo_uuid);
        let mut scripts: Vec<ScriptView> = Vec::new();

        if let Some(rs) = repo_state {
            let mut entries: Vec<_> = rs.scripts.iter().collect();
            entries.sort_by_key(|(_, e)| &e.name);

            for (script_uuid, entry) in entries {
                let eff_update = entry.url_override_update.clone().unwrap_or_else(|| {
                    format!(
                        "{base}/{repo_uuid}/{script_uuid}/{}.meta.js",
                        entry.url_slug
                    )
                });
                let eff_download = entry.url_override_download.clone().unwrap_or_else(|| {
                    format!(
                        "{base}/{repo_uuid}/{script_uuid}/{}.user.js",
                        entry.url_slug
                    )
                });
                scripts.push(ScriptView {
                    uuid: script_uuid.clone(),
                    name: entry.name.clone(),
                    version: entry.version.clone(),
                    description: entry.description.clone(),
                    url_slug: entry.url_slug.clone(),
                    effective_update_url: eff_update,
                    effective_download_url: eff_download,
                    has_override: entry.url_override_update.is_some()
                        || entry.url_override_download.is_some(),
                    missing: entry.missing,
                });
            }
        }
        repos.push(RepoView {
            name: repo_cfg.name.clone(),
            uuid: repo_uuid.clone(),
            scripts,
        });
    }

    let repos_val: Vec<minijinja::Value> = repos
        .iter()
        .map(|r| {
            let scripts_val: Vec<minijinja::Value> = r
                .scripts
                .iter()
                .map(|s| {
                    minijinja::Value::from_object(ScriptViewObj {
                        uuid: s.uuid.clone(),
                        repo_uuid: r.uuid.clone(),
                        name: s.name.clone(),
                        version: s.version.clone(),
                        description: s.description.clone(),
                        url_slug: s.url_slug.clone(),
                        effective_update_url: s.effective_update_url.clone(),
                        effective_download_url: s.effective_download_url.clone(),
                        has_override: s.has_override,
                        missing: s.missing,
                    })
                })
                .collect();
            minijinja::Value::from_object(RepoViewObj {
                name: r.name.clone(),
                uuid: r.uuid.clone(),
                scripts: scripts_val,
            })
        })
        .collect();

    let ctx = context! {
        repos => repos_val,
        flash => q.flash,
    };

    match render(&app.templates, "repo_list.html", ctx) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn edit_form(
    State(app): State<AppState>,
    Path((repo_uuid, script_uuid)): Path<(String, String)>,
) -> Response {
    let state = app.state.read().await;
    let entry = match state
        .repos
        .get(&repo_uuid)
        .and_then(|r| r.scripts.get(&script_uuid))
    {
        Some(e) => e.clone(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    drop(state);

    let ctx = context! {
        repo_uuid => repo_uuid,
        script_uuid => script_uuid,
        name => entry.name,
        url_slug => entry.url_slug,
        url_override_update => entry.url_override_update.unwrap_or_default(),
        url_override_download => entry.url_override_download.unwrap_or_default(),
    };

    match render(&app.templates, "script_edit.html", ctx) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct OverrideForm {
    pub url_override_update: String,
    pub url_override_download: String,
}

pub async fn edit_post(
    State(app): State<AppState>,
    Path((repo_uuid, script_uuid)): Path<(String, String)>,
    Form(form): Form<OverrideForm>,
) -> Response {
    let result = app
        .state
        .write_and_save(|state| {
            if let Some(repo_state) = state.repos.get_mut(&repo_uuid) {
                if let Some(entry) = repo_state.scripts.get_mut(&script_uuid) {
                    entry.url_override_update = non_empty(form.url_override_update.clone());
                    entry.url_override_download = non_empty(form.url_override_download.clone());
                }
            }
        })
        .await;

    if let Err(e) = result {
        tracing::error!(error = %e, "failed to save state after override");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Redirect::to("/admin?flash=saved").into_response()
}

pub async fn script_uuid_form(
    State(app): State<AppState>,
    Path((repo_uuid, script_uuid)): Path<(String, String)>,
) -> Response {
    let state = app.state.read().await;
    let entry = match state
        .repos
        .get(&repo_uuid)
        .and_then(|r| r.scripts.get(&script_uuid))
    {
        Some(e) => e.clone(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    drop(state);

    let ctx = context! {
        kind => "script",
        label => entry.name,
        current_uuid => script_uuid,
        repo_uuid => repo_uuid,
        back_url => "/admin",
        post_action => format!("/admin/scripts/{repo_uuid}/{script_uuid}/uuid"),
    };
    match render(&app.templates, "uuid_edit.html", ctx) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct UuidForm {
    pub new_uuid: String,
}

pub async fn script_uuid_post(
    State(app): State<AppState>,
    Path((repo_uuid, script_uuid)): Path<(String, String)>,
    Form(form): Form<UuidForm>,
) -> Response {
    let new_uuid = form.new_uuid.trim().to_string();
    if new_uuid.is_empty() || new_uuid == script_uuid {
        return Redirect::to("/admin?flash=uuid-unchanged").into_response();
    }

    let result = app
        .state
        .write_and_save(|state| {
            if let Some(repo_state) = state.repos.get_mut(&repo_uuid) {
                if let Some(entry) = repo_state.scripts.remove(&script_uuid) {
                    repo_state.scripts.insert(new_uuid.clone(), entry);
                }
            }
        })
        .await;

    if let Err(e) = result {
        tracing::error!(error = %e, "failed to save state after script UUID rename");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Redirect::to("/admin?flash=uuid-saved").into_response()
}

pub async fn repo_uuid_form(
    State(app): State<AppState>,
    Path(repo_uuid): Path<String>,
) -> Response {
    let repo = match app
        .config
        .repos
        .iter()
        .find(|r| r.uuid.as_deref() == Some(&repo_uuid))
    {
        Some(r) => r.clone(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let ctx = context! {
        kind => "repo",
        label => repo.name,
        current_uuid => repo_uuid,
        repo_uuid => "",
        back_url => "/admin",
        post_action => format!("/admin/repos/{repo_uuid}/uuid"),
    };
    match render(&app.templates, "uuid_edit.html", ctx) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn repo_uuid_post(
    State(app): State<AppState>,
    Path(repo_uuid): Path<String>,
    Form(form): Form<UuidForm>,
) -> Response {
    let new_uuid = form.new_uuid.trim().to_string();
    if new_uuid.is_empty() || new_uuid == repo_uuid {
        return Redirect::to("/admin?flash=uuid-unchanged").into_response();
    }

    // Update state: rename key in repos map
    let result = app
        .state
        .write_and_save(|state| {
            if let Some(repo_state) = state.repos.remove(&repo_uuid) {
                state.repos.insert(new_uuid.clone(), repo_state);
            }
        })
        .await;

    if let Err(e) = result {
        tracing::error!(error = %e, "failed to save state after repo UUID rename");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Update config: rewrite the uuid field and save config.toml
    let mut cfg = (*app.config).clone();
    for repo in &mut cfg.repos {
        if repo.uuid.as_deref() == Some(&repo_uuid) {
            repo.uuid = Some(new_uuid.clone());
            break;
        }
    }
    match toml::to_string_pretty(&cfg) {
        Ok(toml_str) => {
            if let Err(e) = std::fs::write(app.config_path.as_ref(), toml_str) {
                tracing::error!(error = %e, "failed to write config after repo UUID rename");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to serialize config after repo UUID rename");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    tracing::warn!(
        old = repo_uuid,
        new = new_uuid,
        "repo UUID renamed — restart required to take full effect"
    );

    Redirect::to("/admin?flash=uuid-saved").into_response()
}

pub async fn pull_post(State(app): State<AppState>, Path(repo_uuid): Path<String>) -> Response {
    let repo = match app
        .config
        .repos
        .iter()
        .find(|r| r.uuid.as_deref() == Some(&repo_uuid))
    {
        Some(r) => r.clone(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let busy = match app.pull_busy.get(&repo_uuid) {
        Some(b) => b,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if busy.swap(true, Ordering::SeqCst) {
        return Redirect::to("/admin?flash=pull-busy").into_response();
    }

    let app_clone = app.clone();
    tokio::spawn(async move {
        let _guard = PullGuard {
            map: app_clone.pull_busy.clone(),
            key: repo_uuid.clone(),
        };
        if let Err(e) = crate::git::run_git_pull(&repo.local_path, &repo.branch).await {
            tracing::error!(repo = repo.name, error = %e, "admin-triggered git pull failed");
            return;
        }
        if let Err(e) = crate::discovery::scan_repo(&repo, &app_clone).await {
            tracing::error!(repo = repo.name, error = %e, "scan failed after admin pull");
        }
    });

    Redirect::to("/admin?flash=pull-started").into_response()
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

// minijinja object wrappers

#[derive(Debug, Clone)]
struct RepoViewObj {
    name: String,
    uuid: String,
    scripts: Vec<minijinja::Value>,
}

impl minijinja::value::Object for RepoViewObj {
    fn get_value(self: &std::sync::Arc<Self>, key: &minijinja::Value) -> Option<minijinja::Value> {
        match key.as_str()? {
            "name" => Some(minijinja::Value::from(self.name.clone())),
            "uuid" => Some(minijinja::Value::from(self.uuid.clone())),
            "scripts" => Some(minijinja::Value::from(self.scripts.clone())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct ScriptViewObj {
    uuid: String,
    repo_uuid: String,
    name: String,
    version: String,
    description: String,
    url_slug: String,
    effective_update_url: String,
    effective_download_url: String,
    has_override: bool,
    missing: bool,
}

impl minijinja::value::Object for ScriptViewObj {
    fn get_value(self: &std::sync::Arc<Self>, key: &minijinja::Value) -> Option<minijinja::Value> {
        match key.as_str()? {
            "uuid" => Some(minijinja::Value::from(self.uuid.clone())),
            "repo_uuid" => Some(minijinja::Value::from(self.repo_uuid.clone())),
            "name" => Some(minijinja::Value::from(self.name.clone())),
            "version" => Some(minijinja::Value::from(self.version.clone())),
            "description" => Some(minijinja::Value::from(self.description.clone())),
            "url_slug" => Some(minijinja::Value::from(self.url_slug.clone())),
            "effective_update_url" => {
                Some(minijinja::Value::from(self.effective_update_url.clone()))
            }
            "effective_download_url" => {
                Some(minijinja::Value::from(self.effective_download_url.clone()))
            }
            "has_override" => Some(minijinja::Value::from(self.has_override)),
            "missing" => Some(minijinja::Value::from(self.missing)),
            _ => None,
        }
    }
}

struct PullGuard {
    map: std::sync::Arc<std::collections::HashMap<String, std::sync::atomic::AtomicBool>>,
    key: String,
}

impl Drop for PullGuard {
    fn drop(&mut self) {
        if let Some(flag) = self.map.get(&self.key) {
            flag.store(false, Ordering::SeqCst);
        }
    }
}
