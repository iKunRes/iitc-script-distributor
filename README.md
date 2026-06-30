# iitc-script-distributor

A self-hosted distribution service for [IITC](https://iitc.app/) userscripts stored in Git repositories. It serves `.user.js` and `.meta.js` files with `@updateURL` / `@downloadURL` metadata rewritten to point at itself, so userscript managers always update from your instance — regardless of what the upstream repo says.

## Features

- **URL rewriting** — rewrites `@updateURL` and `@downloadURL` on every served file. Missing directives are inserted automatically.
- **`.meta.js` support** — serves lightweight metadata-only files so update checks don't download the full script.
- **Multi-repo** — tracks multiple Git repositories, each with its own webhook secret and glob pattern.
- **Stable UUIDs** — each script gets a persistent UUID derived from its repo-relative path. URLs survive file renames only in the same path.
- **Webhook-triggered pull** — accepts GitHub-style `X-Hub-Signature-256` webhooks and runs `git pull` followed by a rescan.
- **Admin UI** — HTTP Basic Auth–protected web interface for managing URL overrides and triggering manual pulls.
- **Telegram notifications** — optionally sends commit summaries to a Telegram chat after each successful pull.
- **Health endpoint** — `GET /health` for reverse-proxy and uptime monitor use.

## URL scheme

| Endpoint | Description |
|---|---|
| `GET /{repo_uuid}/{script_uuid}/{slug}.user.js` | Full script with rewritten metadata |
| `GET /{repo_uuid}/{script_uuid}/{slug}.meta.js` | Metadata block only (for update checks) |
| `POST /webhook/{repo_uuid}` | GitHub push webhook |
| `GET /admin/` | Script listing (Basic Auth) |
| `GET /health` | Health check |

## Installation

```sh
git clone https://github.com/KunoiSayami/iitc-script-distributor.git
cd iitc-script-distributor
cargo build --release
```

The binary is at `target/release/iitc-script-distributor`.

## Configuration

Copy the example config and edit it:

```sh
cp config.toml.example config.toml
```

```toml
bind            = "0.0.0.0:8080"
public_base_url = "https://scripts.example.com"   # used to build @updateURL / @downloadURL
state_file      = "data/state.json"

[admin]
username = "admin"
password = "$argon2id$v=19$m=19456,t=2,p=1$..."   # PHC hash — see "Hashing the admin password" below

# Optional: Telegram notifications after each successful pull
#[telegram]
#bot_token  = "123456:ABC..."
#send_to    = [-100123456789]     # array or single chat ID

[api]
require_auth = true   # set false to expose GET /api/scripts without auth

[[repos]]
name           = "iitc-community"
git_url        = "https://github.com/IITC-CE/ingress-intel-total-conversion.git"
local_path     = "/srv/scripts/iitc-community"
webhook_secret = "your-github-webhook-secret"
scripts_glob   = "**/*.user.js"
branch         = "master"
```

`uuid` fields in `[[repos]]` are auto-generated and written back to `config.toml` on first start.

## Hashing the admin password

The `[admin] password` field must be an **argon2id PHC hash string**, not a plaintext password. Generate one with the built-in subcommand:

```sh
iitc-script-distributor --hash-password 'your-password-here'
```

This prints a string like:

```
$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>
```

Paste that entire string as the `password` value in `config.toml`.

## Running

```sh
# Start normally (repos must already be cloned)
iitc-script-distributor --config config.toml

# Clone any missing repos before starting
iitc-script-distributor --config config.toml --init-repos
```

## First-time setup

1. Clone your script repository to the configured `local_path`:
   ```sh
   git clone https://github.com/IITC-CE/ingress-intel-total-conversion.git /srv/scripts/iitc-community
   ```
   Or use `--init-repos` to have the service do it.

2. Start the service. It scans all repos and assigns UUIDs on startup.

3. Open `http://localhost:8080/admin/` to see the discovered scripts and their URLs.

4. Configure the GitHub webhook:
   - **Payload URL**: `https://scripts.example.com/webhook/<repo_uuid>`
   - **Content type**: `application/json`
   - **Secret**: matches `webhook_secret` in config
   - **Events**: Just the `push` event

## GitHub webhook setup

Find your repo's UUID in `data/state.json` or `config.toml` after first start, then add the webhook in your repository's **Settings → Webhooks**:

```
Payload URL:  https://scripts.example.com/webhook/<repo_uuid>
Content type: application/json
Secret:       <webhook_secret from config>
Events:       Just the push event
```

To test the webhook manually:

```sh
SECRET="your-webhook-secret"
BODY='{"ref":"refs/heads/master","before":"0000000000000000000000000000000000000000","after":"abc123","commits":[],"compare":""}'
SIG=$(echo -n "$BODY" | openssl dgst -sha256 -hmac "$SECRET" | awk '{print "sha256="$2}')
curl -X POST https://scripts.example.com/webhook/<repo_uuid> \
  -H "X-Hub-Signature-256: $SIG" \
  -H "X-GitHub-Event: push" \
  -H "Content-Type: application/json" \
  -d "$BODY"
```

## Admin interface

The admin UI at `/admin/` (HTTP Basic Auth) lets you:

- View all discovered scripts with their effective `@updateURL` / `@downloadURL`
- Override the URL for any script (useful if you want to point some scripts at an external host)
- Trigger a `git pull` + rescan for any repo without waiting for a webhook

## Deploying behind a reverse proxy

Example nginx snippet:

```nginx
location / {
    proxy_pass http://127.0.0.1:8080;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
}
```

Set `public_base_url` to the public HTTPS address so rewritten URLs are correct.

## Telegram notifications

Add a `[telegram]` section to config. The bot must have permission to post in the target chat:

```toml
[telegram]
bot_token  = "123456:ABC..."
send_to    = [-100123456789]
# api_server = "https://..."   # optional, for custom Bot API servers
```

After each successful `git pull`, a message listing the new commits is sent.

## License

[AGPL-3.0](LICENSE)
