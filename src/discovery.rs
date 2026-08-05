use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::AppState;
use crate::config::RepoConfig;
use crate::scripts::{parse_metadata, slug_from_path};
use crate::state::ScriptEntry;

pub async fn scan_repo(repo: &RepoConfig, app: &AppState) -> anyhow::Result<()> {
    let repo_uuid = repo.uuid.clone().expect("repo must have uuid before scan");
    let patterns: Vec<String> = repo
        .scripts_glob
        .iter()
        .map(|g| format!("{}/{}", repo.local_path, g))
        .collect();
    let local_path = repo.local_path.clone();

    let paths = tokio::task::spawn_blocking(move || {
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        for pattern in patterns {
            let matches = glob::glob(&pattern)
                .map_err(|e| anyhow::anyhow!("invalid glob pattern: {e}"))?
                .filter_map(|entry| match entry {
                    Ok(p) => Some(Ok(p)),
                    Err(e) => {
                        tracing::warn!("glob error: {e}");
                        None
                    }
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            for p in matches {
                if seen.insert(p.clone()) {
                    paths.push(p);
                }
            }
        }
        Ok::<_, anyhow::Error>(paths)
    })
    .await??;

    struct ScannedScript {
        relative_path: String,
        name: String,
        version: String,
        description: String,
        slug: String,
        identity: Option<String>,
    }

    // Resolve the repo root once so symlinked hits can be checked for
    // containment against it.
    let repo_root = tokio::fs::canonicalize(&local_path)
        .await
        .unwrap_or_else(|_| std::path::PathBuf::from(&local_path));

    let mut scanned: Vec<ScannedScript> = Vec::new();
    // Same real file reached via several paths (symlink, or a directory
    // symlink the glob walked into) — keep the first, skip the rest.
    let mut seen_real_paths: HashSet<std::path::PathBuf> = HashSet::new();
    for path in paths {
        let rel = match path.strip_prefix(&local_path) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => {
                tracing::warn!(path = %path.display(), "path outside repo local_path, skipping");
                continue;
            }
        };

        // `strip_prefix` only rejects lexically-outside paths; a symlink inside
        // the repo can still point anywhere on disk, so resolve it and require
        // the target to stay under the repo root. Broken links fail here and
        // are skipped rather than surfacing as unreadable entries later.
        let real_path = match tokio::fs::canonicalize(&path).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to resolve script path, skipping");
                continue;
            }
        };
        if !real_path.starts_with(&repo_root) {
            tracing::warn!(
                path = %path.display(),
                target = %real_path.display(),
                "script resolves outside repo, skipping"
            );
            continue;
        }
        if !seen_real_paths.insert(real_path.clone()) {
            tracing::debug!(
                path = %path.display(),
                target = %real_path.display(),
                "duplicate path for the same file, skipping"
            );
            continue;
        }

        let slug = slug_from_path(&path);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read script, skipping");
                continue;
            }
        };
        let meta = parse_metadata(&content);
        let identity = meta.identity();
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
            identity,
        });
    }

    // Two distinct files can still describe the same script (a committed build
    // artifact next to its source). Keep the shallowest path, then the
    // lexicographically smallest, so the winner is stable across scans and
    // matches the copy a repo usually advertises for install.
    scanned.sort_by(|a, b| {
        let depth = |p: &str| p.matches('/').count();
        depth(&a.relative_path)
            .cmp(&depth(&b.relative_path))
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });
    // Identity of every path seen on disk, including the duplicates dropped
    // below. Entries written before identity tracking existed carry an empty
    // identity, so they are backfilled from this map before matching runs —
    // otherwise a pre-existing duplicate pair could never be recognised as one
    // script and would leak its overrides when its path stopped being served.
    let identity_by_path: HashMap<String, String> = scanned
        .iter()
        .filter_map(|s| Some((s.relative_path.clone(), s.identity.clone()?)))
        .collect();

    let mut seen_identities: HashSet<String> = HashSet::new();
    let mut duplicate_count = 0usize;
    scanned.retain(|s| {
        let Some(identity) = &s.identity else {
            return true;
        };
        if seen_identities.insert(identity.clone()) {
            return true;
        }
        duplicate_count += 1;
        tracing::info!(
            path = s.relative_path,
            identity = identity,
            "duplicate script identity, skipping"
        );
        false
    });

    let found_paths: HashSet<String> = scanned.iter().map(|s| s.relative_path.clone()).collect();
    let count_scanned = scanned.len();

    app.state
        .write_and_save(|state| {
            let repo_state = state.repos.entry(repo_uuid.clone()).or_default();

            // Backfill identities from disk before matching so legacy entries
            // and dropped duplicates participate in the merge below.
            for entry in repo_state.scripts.values_mut() {
                if entry.identity.is_empty() {
                    if let Some(identity) = identity_by_path.get(&entry.relative_path) {
                        entry.identity = identity.clone();
                    }
                }
            }

            // Mark files missing if not found in this scan
            for entry in repo_state.scripts.values_mut() {
                entry.missing = !found_paths.contains(&entry.relative_path);
            }

            let mut new_count = 0usize;
            let mut repaired_count = 0usize;
            for s in scanned {
                // Match on identity first so a script keeps its UUID when it
                // moves, gets symlinked, or its duplicate copy is the one that
                // survives dedup. Fall back to path for scripts whose metadata
                // block carries no usable identity.
                //
                // When earlier scans minted several UUIDs for one script, the
                // survivor decides which already-distributed @updateURL keeps
                // resolving, so it must not be arbitrary. Entries carry no
                // creation time, so age is unrecoverable; rank instead by:
                //   1. the entry already sitting at the path that won dedup —
                //      the copy a repo advertises for install,
                //   2. an entry an admin deliberately touched (override set,
                //      or explicitly disabled),
                //   3. UUID string, purely to stay deterministic.
                let mut matches: Vec<String> = s
                    .identity
                    .as_ref()
                    .map(|identity| {
                        repo_state
                            .scripts
                            .iter()
                            .filter(|(_, e)| &e.identity == identity)
                            .map(|(uuid, _)| uuid.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                matches.sort_by_cached_key(|uuid| {
                    let entry = &repo_state.scripts[uuid];
                    let path_match = entry.relative_path != s.relative_path;
                    let untouched = entry.url_override_update.is_none()
                        && entry.url_override_download.is_none()
                        && !entry.disabled
                        && !entry.rewrite_disabled;
                    (path_match, untouched, uuid.clone())
                });

                let existing_uuid = matches.first().cloned().or_else(|| {
                    repo_state
                        .scripts
                        .iter()
                        .find(|(_, e)| e.relative_path == s.relative_path)
                        .map(|(uuid, _)| uuid.clone())
                });

                if let Some(uuid) = existing_uuid {
                    // Fold any extra UUIDs for this identity into the survivor,
                    // carrying over overrides so admin edits are not lost.
                    for stale in matches.iter().skip(1) {
                        if let Some(old) = repo_state.scripts.remove(stale) {
                            let entry = repo_state.scripts.get_mut(&uuid).unwrap();
                            entry.url_override_update =
                                entry.url_override_update.take().or(old.url_override_update);
                            entry.url_override_download = entry
                                .url_override_download
                                .take()
                                .or(old.url_override_download);
                            entry.disabled = entry.disabled || old.disabled;
                            entry.rewrite_disabled = entry.rewrite_disabled || old.rewrite_disabled;
                            repaired_count += 1;
                            tracing::info!(
                                kept = uuid,
                                removed = stale,
                                path = s.relative_path,
                                "merged duplicate script entry"
                            );
                        }
                    }

                    let entry = repo_state.scripts.get_mut(&uuid).unwrap();
                    entry.relative_path = s.relative_path;
                    entry.name = s.name;
                    entry.version = s.version;
                    entry.description = s.description;
                    entry.url_slug = s.slug;
                    entry.missing = false;
                    if let Some(identity) = s.identity {
                        entry.identity = identity;
                    }
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
                            disabled: false,
                            rewrite_disabled: false,
                            identity: s.identity.unwrap_or_default(),
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
                repaired = repaired_count,
                duplicates = duplicate_count,
                missing = missing_count,
                "scan complete"
            );
        })
        .await
}
