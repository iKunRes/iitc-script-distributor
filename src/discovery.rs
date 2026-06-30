use std::collections::HashSet;
use uuid::Uuid;

use crate::AppState;
use crate::config::RepoConfig;
use crate::scripts::{parse_metadata, slug_from_path};
use crate::state::ScriptEntry;

pub async fn scan_repo(repo: &RepoConfig, app: &AppState) -> anyhow::Result<()> {
    let repo_uuid = repo.uuid.clone().expect("repo must have uuid before scan");
    let pattern = format!("{}/{}", repo.local_path, repo.scripts_glob);
    let local_path = repo.local_path.clone();

    let paths = tokio::task::spawn_blocking(move || {
        glob::glob(&pattern)
            .map_err(|e| anyhow::anyhow!("invalid glob pattern: {e}"))?
            .filter_map(|entry| match entry {
                Ok(p) => Some(Ok(p)),
                Err(e) => {
                    tracing::warn!("glob error: {e}");
                    None
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()
    })
    .await??;

    struct ScannedScript {
        relative_path: String,
        name: String,
        version: String,
        description: String,
        slug: String,
    }

    let mut scanned: Vec<ScannedScript> = Vec::new();
    for path in paths {
        let rel = match path.strip_prefix(&local_path) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => {
                tracing::warn!(path = %path.display(), "path outside repo local_path, skipping");
                continue;
            }
        };
        let slug = slug_from_path(&path);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read script, skipping");
                continue;
            }
        };
        let meta = parse_metadata(&content);
        scanned.push(ScannedScript {
            relative_path: rel,
            name: if meta.name.is_empty() {
                slug.clone()
            } else {
                meta.name
            },
            version: meta.version,
            description: meta.description,
            slug,
        });
    }

    let found_paths: HashSet<String> = scanned.iter().map(|s| s.relative_path.clone()).collect();
    let count_scanned = scanned.len();

    app.state
        .write_and_save(|state| {
            let repo_state = state.repos.entry(repo_uuid.clone()).or_default();

            // Mark files missing if not found in this scan
            for entry in repo_state.scripts.values_mut() {
                entry.missing = !found_paths.contains(&entry.relative_path);
            }

            let mut new_count = 0usize;
            for s in scanned {
                // Find existing UUID for this path (linear scan — repos are small)
                let existing_uuid = repo_state
                    .scripts
                    .iter()
                    .find(|(_, e)| e.relative_path == s.relative_path)
                    .map(|(uuid, _)| uuid.clone());

                if let Some(uuid) = existing_uuid {
                    let entry = repo_state.scripts.get_mut(&uuid).unwrap();
                    entry.name = s.name;
                    entry.version = s.version;
                    entry.description = s.description;
                    entry.url_slug = s.slug;
                    entry.missing = false;
                } else {
                    let uuid = Uuid::new_v4().to_string();
                    repo_state.scripts.insert(
                        uuid,
                        ScriptEntry {
                            relative_path: s.relative_path,
                            name: s.name,
                            version: s.version,
                            description: s.description,
                            url_slug: s.slug,
                            url_override_update: None,
                            url_override_download: None,
                            missing: false,
                        },
                    );
                    new_count += 1;
                }
            }

            let missing_count = repo_state.scripts.values().filter(|e| e.missing).count();
            tracing::info!(
                repo = repo_uuid,
                total = count_scanned,
                new = new_count,
                missing = missing_count,
                "scan complete"
            );
        })
        .await
}
