use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::path::Path as FsPath;

use crate::AppState;

#[derive(Debug, Default)]
pub struct ParsedMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub update_url: Option<String>,
    pub download_url: Option<String>,
}

pub fn parse_metadata(content: &str) -> ParsedMetadata {
    let mut meta = ParsedMetadata::default();
    let mut in_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "// ==UserScript==" {
            in_block = true;
            continue;
        }
        if trimmed == "// ==/UserScript==" {
            break;
        }
        if !in_block {
            continue;
        }
        // Key is "// @key", value is the rest after whitespace
        let without_prefix = match trimmed.strip_prefix("//") {
            Some(s) => s.trim_start(),
            None => continue,
        };
        let mut parts = without_prefix.splitn(2, char::is_whitespace);
        let key = match parts.next() {
            Some(k) => k,
            None => continue,
        };
        let value = parts.next().map(|v| v.trim()).unwrap_or("").to_string();
        match key {
            "@name" => {
                if meta.name.is_empty() {
                    meta.name = value;
                }
            }
            "@version" => {
                if meta.version.is_empty() {
                    meta.version = value;
                }
            }
            "@description" => {
                if meta.description.is_empty() {
                    meta.description = value;
                }
            }
            "@updateURL" if meta.update_url.is_none() => {
                meta.update_url = Some(value);
            }
            "@downloadURL" if meta.download_url.is_none() => {
                meta.download_url = Some(value);
            }
            _ => {}
        }
    }
    meta
}

pub fn slug_from_path(path: &FsPath) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.trim_end_matches(".user"))
        .unwrap_or("script")
        .to_lowercase()
        .replace(' ', "-")
}

pub fn rewrite_userscript(content: &str, update_url: &str, download_url: &str) -> String {
    let mut pre: Vec<&str> = Vec::new();
    let mut block: Vec<String> = Vec::new();
    let mut post: Vec<&str> = Vec::new();
    let mut state = 0u8; // 0=pre, 1=block, 2=post

    let mut saw_update = false;
    let mut saw_download = false;
    let mut injected = false;

    for line in content.lines() {
        let trimmed = line.trim();
        match state {
            0 => {
                if trimmed == "// ==UserScript==" {
                    state = 1;
                    block.push(line.to_string());
                } else {
                    pre.push(line);
                }
            }
            1 => {
                if trimmed == "// ==/UserScript==" {
                    // Insert missing URL directives before closing sentinel
                    if !saw_update {
                        block.push(format!("// @updateURL     {update_url}"));
                    }
                    if !saw_download {
                        block.push(format!("// @downloadURL   {download_url}"));
                    }
                    block.push(line.to_string());
                    state = 2;
                } else if trimmed
                    .strip_prefix("//")
                    .map(|s| s.trim_start().starts_with("@updateURL"))
                    .unwrap_or(false)
                {
                    block.push(format!("// @updateURL     {update_url}"));
                    saw_update = true;
                } else if trimmed
                    .strip_prefix("//")
                    .map(|s| s.trim_start().starts_with("@downloadURL"))
                    .unwrap_or(false)
                {
                    block.push(format!("// @downloadURL   {download_url}"));
                    saw_download = true;
                } else {
                    // Inject missing URL directives before the first @match
                    if !injected
                        && trimmed
                            .strip_prefix("//")
                            .map(|s| s.trim_start().starts_with("@match"))
                            .unwrap_or(false)
                    {
                        if !saw_update {
                            block.push(format!("// @updateURL     {update_url}"));
                            saw_update = true;
                        }
                        if !saw_download {
                            block.push(format!("// @downloadURL   {download_url}"));
                            saw_download = true;
                        }
                        injected = true;
                    }
                    block.push(line.to_string());
                }
            }
            _ => {
                post.push(line);
            }
        }
    }

    let mut result = String::new();
    for line in &pre {
        result.push_str(line);
        result.push('\n');
    }
    for line in &block {
        result.push_str(line);
        result.push('\n');
    }
    for line in &post {
        result.push_str(line);
        result.push('\n');
    }
    result
}

pub fn extract_metadata_block(content: &str, update_url: &str, download_url: &str) -> String {
    let mut block: Vec<String> = Vec::new();
    let mut in_block = false;
    let mut saw_update = false;
    let mut saw_download = false;
    let mut injected = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "// ==UserScript==" {
            in_block = true;
            block.push(line.to_string());
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed == "// ==/UserScript==" {
            if !saw_update {
                block.push(format!("// @updateURL     {update_url}"));
            }
            if !saw_download {
                block.push(format!("// @downloadURL   {download_url}"));
            }
            block.push(line.to_string());
            break;
        }
        if trimmed
            .strip_prefix("//")
            .map(|s| s.trim_start().starts_with("@updateURL"))
            .unwrap_or(false)
        {
            block.push(format!("// @updateURL     {update_url}"));
            saw_update = true;
        } else if trimmed
            .strip_prefix("//")
            .map(|s| s.trim_start().starts_with("@downloadURL"))
            .unwrap_or(false)
        {
            block.push(format!("// @downloadURL   {download_url}"));
            saw_download = true;
        } else {
            // Inject missing URL directives before the first @match
            if !injected
                && trimmed
                    .strip_prefix("//")
                    .map(|s| s.trim_start().starts_with("@match"))
                    .unwrap_or(false)
            {
                if !saw_update {
                    block.push(format!("// @updateURL     {update_url}"));
                    saw_update = true;
                }
                if !saw_download {
                    block.push(format!("// @downloadURL   {download_url}"));
                    saw_download = true;
                }
                injected = true;
            }
            block.push(line.to_string());
        }
    }

    let mut result = block.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

pub async fn serve_userscript(
    State(app): State<AppState>,
    Path((repo_uuid, script_uuid, slug)): Path<(String, String, String)>,
) -> Response {
    let is_meta = if slug.ends_with(".meta.js") {
        true
    } else if slug.ends_with(".user.js") {
        false
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let slug_base = if is_meta {
        slug.trim_end_matches(".meta.js")
    } else {
        slug.trim_end_matches(".user.js")
    };

    let (relative_path, update_url, download_url) = {
        let state = app.state.read().await;
        let repo_state = match state.repos.get(&repo_uuid) {
            Some(r) => r,
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        let entry = match repo_state.scripts.get(&script_uuid) {
            Some(e) => e,
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        if entry.missing {
            return StatusCode::NOT_FOUND.into_response();
        }
        if entry.disabled {
            return StatusCode::NOT_FOUND.into_response();
        }
        if entry.url_slug != slug_base {
            return StatusCode::NOT_FOUND.into_response();
        }

        let base = app.config.public_base_url.trim_end_matches('/');
        let update_url = entry.url_override_update.clone().unwrap_or_else(|| {
            format!(
                "{base}/{repo_uuid}/{script_uuid}/{}.meta.js",
                entry.url_slug
            )
        });
        let download_url = entry.url_override_download.clone().unwrap_or_else(|| {
            format!(
                "{base}/{repo_uuid}/{script_uuid}/{}.user.js",
                entry.url_slug
            )
        });
        (entry.relative_path.clone(), update_url, download_url)
    };

    let repo_config = match app
        .config
        .repos
        .iter()
        .find(|r| r.uuid.as_deref().map(|u| u == repo_uuid).unwrap_or(false))
    {
        Some(r) => r,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let file_path = format!("{}/{}", repo_config.local_path, relative_path);
    let content = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(path = file_path, error = %e, "failed to read script file");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    let body = if is_meta {
        extract_metadata_block(&content, &update_url, &download_url)
    } else {
        rewrite_userscript(&content, &update_url, &download_url)
    };

    let mut response = Response::new(axum::body::Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const SAMPLE: &str = r#"// ==UserScript==
// @name         Draw Tools
// @version      0.9.0
// @description  Draw tools for IITC
// @updateURL    https://old.example.com/draw-tools.meta.js
// @downloadURL  https://old.example.com/draw-tools.user.js
// ==/UserScript==
(function() { /* code */ })();
"#;

    const SAMPLE_NO_URLS: &str = r#"// ==UserScript==
// @name         Draw Tools
// @version      0.9.0
// @description  Draw tools
// @match        https://intel.ingress.com/*
// ==/UserScript==
(function() {})();
"#;

    #[test]
    fn test_parse_metadata() {
        let meta = parse_metadata(SAMPLE);
        assert_eq!(meta.name, "Draw Tools");
        assert_eq!(meta.version, "0.9.0");
        assert_eq!(meta.description, "Draw tools for IITC");
        assert_eq!(
            meta.update_url.as_deref(),
            Some("https://old.example.com/draw-tools.meta.js")
        );
    }

    #[test]
    fn test_rewrite_replaces_existing_urls() {
        let out = rewrite_userscript(
            SAMPLE,
            "https://new.example.com/draw-tools.meta.js",
            "https://new.example.com/draw-tools.user.js",
        );
        assert!(out.contains("@updateURL     https://new.example.com/draw-tools.meta.js"));
        assert!(out.contains("@downloadURL   https://new.example.com/draw-tools.user.js"));
        assert!(!out.contains("old.example.com"));
        assert!(out.contains("/* code */"));
    }

    #[test]
    fn test_rewrite_inserts_missing_urls() {
        let out = rewrite_userscript(
            SAMPLE_NO_URLS,
            "https://new.example.com/draw-tools.meta.js",
            "https://new.example.com/draw-tools.user.js",
        );
        assert!(out.contains("@updateURL     https://new.example.com/draw-tools.meta.js"));
        assert!(out.contains("@downloadURL   https://new.example.com/draw-tools.user.js"));
        // URL directives must appear before @match
        let update_pos = out.find("@updateURL").unwrap();
        let match_pos = out.find("@match").unwrap();
        assert!(
            update_pos < match_pos,
            "@updateURL must appear before @match"
        );
    }

    #[test]
    fn test_extract_metadata_block_only() {
        let out = extract_metadata_block(
            SAMPLE,
            "https://new.example.com/draw-tools.meta.js",
            "https://new.example.com/draw-tools.user.js",
        );
        assert!(out.contains("// ==UserScript=="));
        assert!(out.contains("// ==/UserScript=="));
        assert!(!out.contains("/* code */"));
        assert!(out.contains("@updateURL     https://new.example.com/draw-tools.meta.js"));
    }

    #[test]
    fn test_slug_from_path() {
        assert_eq!(
            slug_from_path(Path::new("plugins/draw-tools.user.js")),
            "draw-tools"
        );
        assert_eq!(slug_from_path(Path::new("My Plugin.user.js")), "my-plugin");
    }
}
