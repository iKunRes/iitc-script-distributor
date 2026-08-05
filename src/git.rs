use crate::github_app::basic_auth_header_value;

fn auth_args(gh_token: Option<&str>) -> Vec<String> {
    match gh_token {
        Some(token) => vec![
            "-c".to_string(),
            format!("http.extraHeader={}", basic_auth_header_value(token)),
        ],
        None => Vec::new(),
    }
}

pub async fn run_git_pull(
    local_path: &str,
    branch: &str,
    gh_token: Option<&str>,
) -> anyhow::Result<()> {
    let auth = auth_args(gh_token);

    let fetch_out = tokio::process::Command::new("git")
        .args(["-C", local_path])
        .args(&auth)
        .args(["fetch", "origin", branch])
        .output()
        .await?;
    if !fetch_out.status.success() {
        let stderr = String::from_utf8_lossy(&fetch_out.stderr);
        anyhow::bail!("git fetch failed: {stderr}");
    }

    let reset_out = tokio::process::Command::new("git")
        .args([
            "-C",
            local_path,
            "reset",
            "--hard",
            &format!("origin/{branch}"),
        ])
        .output()
        .await?;
    if !reset_out.status.success() {
        let stderr = String::from_utf8_lossy(&reset_out.stderr);
        anyhow::bail!("git reset failed: {stderr}");
    }

    tracing::info!(path = local_path, branch, "git pull succeeded");
    Ok(())
}

pub async fn run_git_clone(
    git_url: &str,
    local_path: &str,
    branch: &str,
    gh_token: Option<&str>,
) -> anyhow::Result<()> {
    let auth = auth_args(gh_token);

    let out = tokio::process::Command::new("git")
        .args(&auth)
        .args(["clone", "--branch", branch, git_url, local_path])
        .output()
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("git clone failed: {stderr}");
    }
    tracing::info!(url = git_url, path = local_path, "git clone succeeded");
    Ok(())
}
