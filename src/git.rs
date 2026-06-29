pub async fn run_git_pull(local_path: &str, branch: &str) -> anyhow::Result<()> {
    let out = tokio::process::Command::new("git")
        .args(["-C", local_path, "pull", "--ff-only", "origin", branch])
        .output()
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("git pull failed: {stderr}");
    }
    tracing::info!(path = local_path, branch, "git pull succeeded");
    Ok(())
}

pub async fn run_git_clone(git_url: &str, local_path: &str, branch: &str) -> anyhow::Result<()> {
    let out = tokio::process::Command::new("git")
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
