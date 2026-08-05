use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Context;
use argon2::PasswordHasher;
use clap::Parser;
use github_webhook_notification::server::{Command, process_send_message};
use tokio::sync::mpsc;
use tracing::Level;
use tracing_subscriber::fmt::time::SystemTime;

mod admin;
mod config;
mod discovery;
mod git;
mod github_app;
mod router;
mod scripts;
mod state;
mod webhook;

use config::{Config, RepoAuthConfig, RepoConfig};
use github_app::GithubAppAuth;
use state::SharedState;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub config_path: Arc<PathBuf>,
    pub state: Arc<SharedState>,
    pub templates: Arc<minijinja::Environment<'static>>,
    pub pull_busy: Arc<HashMap<String, AtomicBool>>,
    pub bot_tx: Option<mpsc::Sender<Command>>,
    pub telegram_send_to: Arc<Vec<i64>>,
    pub github_app: Option<Arc<GithubAppAuth>>,
}

/// Resolves the GitHub App installation token for `repo`, if it's configured
/// to use `[repos.auth] mode = "github_app"`. Returns `None` for repos that
/// don't opt in, which leaves git operations unauthenticated as before.
pub async fn resolve_gh_token(app: &AppState, repo: &RepoConfig) -> anyhow::Result<Option<String>> {
    match &repo.auth {
        Some(RepoAuthConfig::GithubApp { owner, repo: name }) => {
            let auth = app.github_app.as_ref().context(
                "repo uses auth.mode = \"github_app\" but no [github_app] config is present",
            )?;
            let token = auth.get_token(owner, name).await?;
            Ok(Some(token))
        }
        None => Ok(None),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "iitc-script-distributor",
    about = "Serve IITC userscripts from Git repos"
)]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    #[arg(long, help = "Clone missing repo local_paths before starting")]
    init_repos: bool,

    #[arg(
        long,
        help = "Hash a password with argon2id and print the PHC string, then exit",
        value_name = "PASSWORD"
    )]
    hash_password: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let under_systemd = std::env::var_os("JOURNAL_STREAM").is_some();
    let builder = tracing_subscriber::fmt().with_max_level(Level::INFO);
    if under_systemd {
        builder.without_time().init();
    } else {
        builder.with_timer(SystemTime).init();
    }

    let args = Args::parse();

    if let Some(password) = args.hash_password {
        use argon2::Argon2;
        let hash = Argon2::default()
            .hash_password(password.as_bytes())
            .map_err(|e| anyhow::anyhow!("argon2 error: {e}"))?;
        println!("{hash}");
        return Ok(());
    }

    let mut cfg = config::load_config(&args.config)?;

    // Create state dir if needed
    if let Some(parent) = std::path::Path::new(&cfg.state_file).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let shared_state = Arc::new(SharedState::load(&cfg.state_file)?);

    let github_app = cfg
        .github_app
        .as_ref()
        .map(|g| {
            let private_key = std::fs::read_to_string(&g.private_key_path).with_context(|| {
                format!(
                    "failed to read GitHub App private key at {}",
                    g.private_key_path
                )
            })?;
            GithubAppAuth::new(g.app_id, &private_key).map(Arc::new)
        })
        .transpose()?;

    // Resolve each repo's UUID from the state file (the source of truth), so
    // UUIDs stay stable even when config.toml is rewritten or regenerated.
    for repo in &mut cfg.repos {
        let uuid = shared_state.resolve_repo_uuid(repo.identity()).await?;
        repo.set_uuid(uuid);
    }

    // Optionally clone repos whose local_path doesn't exist. Clone failures are
    // non-fatal: one unreachable remote must not block the whole service.
    if args.init_repos {
        for repo in &cfg.repos {
            let path = &repo.local_path;
            if !std::path::Path::new(path).exists() {
                let gh_token = match &repo.auth {
                    Some(RepoAuthConfig::GithubApp { owner, repo: name }) => match &github_app {
                        Some(auth) => match auth.get_token(owner, name).await {
                            Ok(token) => Some(token),
                            Err(e) => {
                                tracing::error!(path, error = %e, "failed to obtain GitHub App token, skipping repo");
                                continue;
                            }
                        },
                        None => {
                            tracing::error!(
                                path,
                                "repo uses auth.mode = \"github_app\" but no [github_app] config is present, skipping repo"
                            );
                            continue;
                        }
                    },
                    None => None,
                };

                tracing::info!(path, "cloning repo");
                if let Err(e) =
                    git::run_git_clone(&repo.git_url, path, &repo.branch, gh_token.as_deref()).await
                {
                    tracing::error!(path, error = %e, "git clone failed, skipping repo");
                }
            }
        }
    }

    // Build per-repo busy flags
    let pull_busy: HashMap<String, AtomicBool> = cfg
        .repos
        .iter()
        .filter_map(|r| r.uuid.clone())
        .map(|u| (u, AtomicBool::new(false)))
        .collect();

    // Telegram setup
    let (bot_tx, telegram_send_to) = if let Some(tg) = &cfg.telegram {
        let (tx, rx) = mpsc::channel::<Command>(1024);
        let token = tg.bot_token.clone();
        let api_server = tg.api_server.clone();
        tokio::spawn(async move {
            if let Err(e) = process_send_message(token, api_server, rx).await {
                tracing::error!(error = %e, "Telegram sender exited with error");
            }
        });
        let send_to = tg.send_to.clone();
        (Some(tx), send_to)
    } else {
        (None, Vec::new())
    };

    let templates = Arc::new(admin::templates::build_env());

    let app_state = AppState {
        config: Arc::new(cfg.clone()),
        config_path: Arc::new(args.config.clone()),
        state: shared_state.clone(),
        templates,
        pull_busy: Arc::new(pull_busy),
        bot_tx,
        telegram_send_to: Arc::new(telegram_send_to),
        github_app,
    };

    // Initial scan of existing repos
    for repo in &cfg.repos {
        if std::path::Path::new(&repo.local_path).is_dir() {
            if let Err(e) = discovery::scan_repo(repo, &app_state).await {
                tracing::error!(repo = repo.name, error = %e, "initial scan failed");
            }
        } else {
            tracing::warn!(
                repo = repo.name,
                path = repo.local_path,
                "local_path does not exist, skipping initial scan"
            );
        }
    }

    let router = router::build_router(app_state);
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(bind = cfg.bind, "server started");
    axum::serve(listener, router).await?;

    Ok(())
}
