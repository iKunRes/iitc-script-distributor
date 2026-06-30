use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use github_webhook_notification::datastructures::CommandBundle;
use github_webhook_notification::server::Command;
use github_webhook_notification::{GitHubPingEvent, GitHubPushEvent, compute_signature};
use subtle::ConstantTimeEq;

use crate::AppState;

pub async fn handle_webhook(
    State(app): State<AppState>,
    Path(repo_uuid): Path<String>,
    request: Request,
) -> impl IntoResponse {
    let repo = match app
        .config
        .repos
        .iter()
        .find(|r| r.uuid.as_deref() == Some(&repo_uuid))
    {
        Some(r) => r.clone(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let (parts, body) = request.into_parts();

    let body_bytes = match axum::body::to_bytes(body, 262_144).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // HMAC verification
    let expected = compute_signature(repo.webhook_secret.as_bytes(), &body_bytes);
    let provided = parts
        .headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if expected.as_bytes().ct_eq(provided.as_bytes()).unwrap_u8() == 0 {
        tracing::warn!(repo = repo.name, "webhook HMAC mismatch");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let event_type = parts
        .headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match event_type {
        "ping" => {
            let ping =
                serde_json::from_slice::<GitHubPingEvent>(&body_bytes).unwrap_or_else(|_| {
                    // GitHubPingEvent always has zen; default gracefully
                    serde_json::from_str(r#"{"zen":""}"#).unwrap()
                });
            return (StatusCode::OK, ping.zen().to_string()).into_response();
        }
        "push" => {}
        other => {
            tracing::debug!(event = other, "unsupported GitHub event, ignoring");
            return StatusCode::BAD_REQUEST.into_response();
        }
    }

    let event = match serde_json::from_slice::<GitHubPushEvent>(&body_bytes) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "failed to parse push event");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // Skip branch create/delete (all-zero SHA)
    let all_zeros = |s: &str| s.chars().all(|c| c == '0');
    if all_zeros(event.after()) || all_zeros(event.before()) {
        return StatusCode::NO_CONTENT.into_response();
    }

    // Check busy flag
    if let Some(busy) = app.pull_busy.get(&repo_uuid) {
        if busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
            tracing::warn!(
                repo = repo.name,
                "pull already in progress, ignoring webhook"
            );
            return StatusCode::ACCEPTED.into_response();
        }
    } else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let app_clone = app.clone();
    let event_text = event.to_string();
    tokio::spawn(async move {
        let _guard = BusyGuard {
            map: &app_clone.pull_busy,
            key: repo_uuid.clone(),
        };

        if let Err(e) = crate::git::run_git_pull(&repo.local_path, &repo.branch).await {
            tracing::error!(repo = repo.name, error = %e, "git pull failed");
            return;
        }

        if let Err(e) = crate::discovery::scan_repo(&repo, &app_clone).await {
            tracing::error!(repo = repo.name, error = %e, "scan failed after pull");
        }

        if let Some(tx) = &app_clone.bot_tx {
            let bundle =
                CommandBundle::new(app_clone.telegram_send_to.as_ref().clone(), event_text);
            if let Err(e) = tx.send(Command::Bundle(bundle)).await {
                tracing::warn!(error = %e, "failed to send Telegram notification");
            }
        }
    });

    StatusCode::ACCEPTED.into_response()
}

struct BusyGuard<'a> {
    map: &'a std::collections::HashMap<String, std::sync::atomic::AtomicBool>,
    key: String,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        if let Some(flag) = self.map.get(&self.key) {
            flag.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}
