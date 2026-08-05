use std::time::{Duration, Instant};

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use dashmap::DashMap;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};

const JWT_TTL_SECS: i64 = 9 * 60;
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(2 * 60);

#[derive(Serialize)]
struct Claims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(Deserialize)]
struct InstallationResponse {
    id: u64,
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    token: String,
}

/// Builds the `http.extraHeader` config value for a GitHub App installation token,
/// suitable for `git -c http.extraHeader=<value>`, so the token never appears in
/// process listings or in the repo's `git_url`.
pub fn basic_auth_header_value(token: &str) -> String {
    let basic = BASE64.encode(format!("x-access-token:{token}"));
    format!("AUTHORIZATION: basic {basic}")
}

pub struct GithubAppAuth {
    app_id: u64,
    encoding_key: EncodingKey,
    client: reqwest::Client,
    installation_cache: DashMap<String, u64>,
    token_cache: DashMap<u64, (String, Instant)>,
}

impl GithubAppAuth {
    pub fn new(app_id: u64, private_key_pem: &str) -> anyhow::Result<Self> {
        let encoding_key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
            .context("Invalid GitHub App private key (expected PEM-encoded RSA key)")?;
        Ok(Self {
            app_id,
            encoding_key,
            client: reqwest::Client::builder()
                .user_agent("iitc-script-distributor")
                .build()
                .context("Failed to build HTTP client")?,
            installation_cache: DashMap::new(),
            token_cache: DashMap::new(),
        })
    }

    fn mint_jwt(&self) -> anyhow::Result<String> {
        let now = unix_now();
        let claims = Claims {
            iat: now - 60,
            exp: now + JWT_TTL_SECS,
            iss: self.app_id.to_string(),
        };
        encode(&Header::new(Algorithm::RS256), &claims, &self.encoding_key)
            .context("Failed to sign GitHub App JWT")
    }

    async fn resolve_installation_id(&self, owner: &str, repo: &str) -> anyhow::Result<u64> {
        if let Some(id) = self.installation_cache.get(owner) {
            return Ok(*id);
        }

        let jwt = self.mint_jwt()?;
        let url = format!("https://api.github.com/repos/{owner}/{repo}/installation");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(jwt)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .with_context(|| format!("Failed to resolve installation for {owner}/{repo}"))?
            .error_for_status()
            .with_context(|| format!("GitHub API error resolving installation for {owner}/{repo}"))?
            .json::<InstallationResponse>()
            .await
            .context("Failed to parse installation response")?;

        self.installation_cache.insert(owner.to_owned(), resp.id);
        Ok(resp.id)
    }

    async fn fetch_installation_token(&self, installation_id: u64) -> anyhow::Result<String> {
        let jwt = self.mint_jwt()?;
        let url =
            format!("https://api.github.com/app/installations/{installation_id}/access_tokens");
        let resp = self
            .client
            .post(&url)
            .bearer_auth(jwt)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .context("Failed to request installation access token")?
            .error_for_status()
            .context("GitHub API error requesting installation access token")?
            .json::<AccessTokenResponse>()
            .await
            .context("Failed to parse installation access token response")?;

        // GitHub installation tokens are always valid for 1 hour; track a conservative
        // local expiry rather than parsing the response's `expires_at` timestamp
        // (avoids adding a datetime-parsing dependency).
        let expiry = Instant::now() + Duration::from_secs(55 * 60);
        self.token_cache
            .insert(installation_id, (resp.token.clone(), expiry));
        Ok(resp.token)
    }

    /// Returns a valid installation token for `owner/repo`, minting or refreshing as needed.
    pub async fn get_token(&self, owner: &str, repo: &str) -> anyhow::Result<String> {
        let installation_id = self.resolve_installation_id(owner, repo).await?;

        if let Some(entry) = self.token_cache.get(&installation_id) {
            let (token, expiry) = entry.value();
            if Instant::now() + TOKEN_REFRESH_MARGIN < *expiry {
                return Ok(token.clone());
            }
        }

        self.fetch_installation_token(installation_id).await
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs() as i64
}
