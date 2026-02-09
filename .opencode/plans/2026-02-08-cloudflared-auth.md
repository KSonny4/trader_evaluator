# Plan: Cloudflared Tunnel + HTTP Basic Auth for Dashboard

**Date:** 2026-02-08
**Branch:** `feature/cloudflared-auth`
**Worktree:** `.worktrees/cloudflared-auth` (already created)
**Goal:** Expose the dashboard at `sniper.pkubelka.cz` via cloudflared tunnel with HTTP Basic Auth protection.

## Task 1: Add `auth_password` to Web config

**Files:** `crates/common/src/config.rs`, `config/default.toml`

### config.rs change

Add `auth_password: Option<String>` to the `Web` struct:

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct Web {
    pub port: u16,
    pub host: String,
    pub auth_password: Option<String>,
}
```

### default.toml change

Update `[web]` section:

```toml
[web]
port = 8080
host = "127.0.0.1"
auth_password = "recognize-parade-finalist-flatbed-stumble"
```

Note: `host` changes from `0.0.0.0` to `127.0.0.1` — only accessible via cloudflared tunnel, not directly from outside.

### Test update

Update `test_web_config_section` to also verify `auth_password`:

```rust
#[test]
fn test_web_config_section() {
    let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
    let web = config.web.expect("web section should be present");
    assert_eq!(web.port, 8080);
    assert_eq!(web.host, "127.0.0.1");
    assert!(web.auth_password.is_some());
}
```

**Verify:** `cargo test -p common`

---

## Task 2: Add HTTP Basic Auth middleware to axum

**Files:** `crates/web/Cargo.toml`, `crates/web/src/main.rs`

### Cargo.toml — add `base64` dep

```toml
[dependencies]
# ... existing deps ...
base64 = "0.22"
```

Also add to workspace deps in root `Cargo.toml`:

```toml
base64 = "0.22"
```

And in web's Cargo.toml use workspace:

```toml
base64 = { workspace = true }
```

### main.rs — add auth middleware

Add a Tower middleware layer that checks Basic Auth on every request. The approach:

1. Add `auth_password: Option<String>` to `AppState`
2. Create an async middleware function `basic_auth_middleware` 
3. Apply it as a layer on the router

```rust
use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::middleware::{self, Next};
use base64::Engine as _;

/// Basic auth middleware — returns 401 if password doesn't match.
async fn basic_auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let password = match &state.auth_password {
        Some(pw) => pw,
        None => return next.run(request).await, // no auth configured
    };

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let authenticated = auth_header
        .and_then(|h| h.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|decoded| {
            // Format is "username:password" — we only check the password part
            decoded
                .split_once(':')
                .map_or(false, |(_, pw)| pw == password)
        })
        .unwrap_or(false);

    if authenticated {
        next.run(request).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"Evaluator Dashboard\"")
            .body(Body::from("Unauthorized"))
            .unwrap()
    }
}
```

### AppState change

```rust
pub struct AppState {
    pub db_path: PathBuf,
    pub auth_password: Option<String>,
}
```

### Router change — apply middleware

Update `create_router_with_state` to wrap with auth:

```rust
pub fn create_router_with_state(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/partials/status", get(status_partial))
        .route("/partials/funnel", get(funnel_partial))
        .route("/partials/markets", get(markets_partial))
        .route("/partials/wallets", get(wallets_partial))
        .route("/partials/tracking", get(tracking_partial))
        .route("/partials/paper", get(paper_partial))
        .route("/partials/rankings", get(rankings_partial))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            basic_auth_middleware,
        ))
        .with_state(state)
}
```

### main() change

Read auth_password from config:

```rust
let auth_password = config.web.as_ref().and_then(|w| w.auth_password.clone());

let state = Arc::new(AppState {
    db_path,
    auth_password,
});
```

### Test updates

Update `create_test_app()` to include `auth_password: None` (no auth in tests by default):

```rust
fn create_test_app() -> Router {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let db = Database::open(path.to_str().unwrap()).unwrap();
    db.run_migrations().unwrap();
    drop(db);
    std::mem::forget(tmp);

    let state = Arc::new(AppState {
        db_path: path,
        auth_password: None, // no auth in tests
    });
    create_router_with_state(state)
}
```

Add a helper for auth-enabled test app:

```rust
fn create_test_app_with_auth(password: &str) -> Router {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let db = Database::open(path.to_str().unwrap()).unwrap();
    db.run_migrations().unwrap();
    drop(db);
    std::mem::forget(tmp);

    let state = Arc::new(AppState {
        db_path: path,
        auth_password: Some(password.to_string()),
    });
    create_router_with_state(state)
}

fn basic_auth_header(user: &str, pass: &str) -> String {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", user, pass));
    format!("Basic {}", encoded)
}
```

New tests:

```rust
#[tokio::test]
async fn test_auth_returns_401_without_credentials() {
    let app = create_test_app_with_auth("secret");
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_returns_401_with_wrong_password() {
    let app = create_test_app_with_auth("secret");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("Authorization", basic_auth_header("admin", "wrong"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_returns_200_with_correct_password() {
    let app = create_test_app_with_auth("secret");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("Authorization", basic_auth_header("admin", "secret"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_disabled_when_no_password() {
    let app = create_test_app(); // auth_password: None
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_partials_also_protected() {
    let app = create_test_app_with_auth("secret");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/partials/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_www_authenticate_header_present() {
    let app = create_test_app_with_auth("secret");
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let www_auth = response
        .headers()
        .get("www-authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(www_auth.contains("Basic"));
}
```

**Verify:** `cargo test -p web` — all existing tests should still pass (they use `create_test_app()` with `auth_password: None`), plus 6 new auth tests.

---

## Task 3: Create `deploy/setup-cloudflared.sh`

**File:** `deploy/setup-cloudflared.sh` (NEW)

```bash
#!/usr/bin/env bash
set -euo pipefail

# Install cloudflared and create a tunnel for the evaluator dashboard.
# Run this on the target server (ubuntu@3.8.206.244).
#
# Prerequisites:
#   - Must be run interactively (cloudflared login opens a browser URL)
#
# Usage:
#   bash deploy/setup-cloudflared.sh

TUNNEL_NAME="${TUNNEL_NAME:-evaluator-dashboard}"
HOSTNAME="${HOSTNAME:-sniper.pkubelka.cz}"
LOCAL_SERVICE="http://localhost:8080"

echo "=== Installing cloudflared ==="
# Add Cloudflare's GPG key and repo
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg | sudo tee /usr/share/keyrings/cloudflare-main.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared $(lsb_release -cs) main" | \
    sudo tee /etc/apt/sources.list.d/cloudflared.list
sudo apt-get update -y
sudo apt-get install -y cloudflared

echo ""
echo "=== Authenticating with Cloudflare ==="
echo "This will open a URL — paste it in your browser to authorize."
cloudflared tunnel login

echo ""
echo "=== Creating tunnel: $TUNNEL_NAME ==="
cloudflared tunnel create "$TUNNEL_NAME"

# Get the tunnel ID
TUNNEL_ID=$(cloudflared tunnel list --name "$TUNNEL_NAME" --output json | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['id'])")
CRED_FILE="$HOME/.cloudflared/${TUNNEL_ID}.json"

echo "Tunnel ID: $TUNNEL_ID"
echo "Credentials: $CRED_FILE"

echo ""
echo "=== Writing config ==="
sudo mkdir -p /etc/cloudflared
sudo tee /etc/cloudflared/config.yml > /dev/null <<EOF
tunnel: ${TUNNEL_ID}
credentials-file: ${CRED_FILE}

ingress:
  - hostname: ${HOSTNAME}
    service: ${LOCAL_SERVICE}
  - service: http_status:404
EOF

echo ""
echo "=== Creating DNS route ==="
cloudflared tunnel route dns "$TUNNEL_NAME" "$HOSTNAME"

echo ""
echo "=== Installing as systemd service ==="
sudo cloudflared service install
sudo systemctl enable cloudflared
sudo systemctl start cloudflared

echo ""
echo "=== Done ==="
echo "Tunnel $TUNNEL_NAME is running."
echo "Dashboard should be accessible at https://$HOSTNAME"
echo ""
echo "Verify with: sudo systemctl status cloudflared"
echo "Logs: sudo journalctl -u cloudflared -f"
```

---

## Task 4: Update `deploy/setup-evaluator.sh`

**File:** `deploy/setup-evaluator.sh`

Add installation of `web.service` alongside `evaluator.service`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# One-time server setup for the evaluator service.
# Expected to be run on the target Ubuntu box.

REMOTE_DIR="${REMOTE_DIR:-/opt/evaluator}"

sudo useradd --system --home "$REMOTE_DIR" --shell /usr/sbin/nologin evaluator 2>/dev/null || true

sudo mkdir -p "$REMOTE_DIR"/{data,config}
sudo chown -R evaluator:evaluator "$REMOTE_DIR"

sudo apt-get update -y
sudo apt-get install -y sqlite3 ca-certificates

sudo install -m 0644 -o root -g root deploy/systemd/evaluator.service /etc/systemd/system/evaluator.service
sudo install -m 0644 -o root -g root deploy/systemd/web.service /etc/systemd/system/web.service
sudo systemctl daemon-reload
sudo systemctl enable evaluator
sudo systemctl enable web

echo "OK: evaluator user + directories + systemd services (evaluator + web) installed."
echo "Next: copy binary/config to $REMOTE_DIR and run:"
echo "  sudo systemctl start evaluator"
echo "  sudo systemctl start web"
echo ""
echo "For cloudflared tunnel setup, run: bash deploy/setup-cloudflared.sh"
```

---

## Task 5: Update Makefile SERVER placeholder

**File:** `Makefile`

Update the `SERVER` default to use the actual server IP with key file:

No change needed — the SERVER variable is passed at deploy time and can stay as placeholder. The key file is handled by the user's SSH config or deploy.sh.

Actually, no Makefile changes needed — deploy already handles `web` binary and `systemctl restart web`.

---

## Execution Order

1. Task 1: Config changes (`config.rs` + `default.toml`) → `cargo test -p common`
2. Task 2: Auth middleware in `main.rs` + `Cargo.toml` changes → `cargo test -p web`
3. Task 3: Create `deploy/setup-cloudflared.sh`
4. Task 4: Update `deploy/setup-evaluator.sh`
5. Full verification: `cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
6. Commit all changes
7. Deploy to server: `make deploy SERVER=ubuntu@3.8.206.244`
8. SSH to server, run `bash deploy/setup-cloudflared.sh` interactively
9. Verify `https://sniper.pkubelka.cz` shows login prompt and works with the password

## Notes

- The password is stored in plaintext in `default.toml` — this is fine for a personal project on a private server. The config file is not publicly accessible.
- `127.0.0.1` binding means the dashboard is ONLY accessible through the cloudflared tunnel, not directly on port 8080 from outside.
- Basic auth over cloudflared is secure — Cloudflare terminates TLS at their edge, and the tunnel is encrypted end-to-end.
- The `base64` crate is already in the dependency tree (used by reqwest) — adding it as a direct dep pulls nothing new.
