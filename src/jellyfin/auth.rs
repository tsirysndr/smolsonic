use crate::db::Db;
use actix_web::{dev::Payload, web, FromRequest, HttpRequest};
use anyhow::Result;
use futures::future::LocalBoxFuture;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use super::JellyfinState;

/// Parsed `X-Emby-Authorization` / `Authorization: MediaBrowser …` header.
/// Jellyfin clients send a comma-separated list of `Key="Value"` pairs.
#[derive(Debug, Default, Clone)]
pub struct EmbyAuth {
    pub client: Option<String>,
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub version: Option<String>,
    pub token: Option<String>,
}

pub fn parse_emby_auth_header(value: &str) -> EmbyAuth {
    // Strip optional scheme prefix.
    let body = value
        .strip_prefix("MediaBrowser ")
        .or_else(|| value.strip_prefix("Emby "))
        .unwrap_or(value);

    let mut out = EmbyAuth::default();
    let mut buf = String::new();
    let mut in_quotes = false;
    let mut chars = body.chars().peekable();
    let mut parts: Vec<String> = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                parts.push(std::mem::take(&mut buf));
            }
            _ => buf.push(c),
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }

    let pairs: HashMap<String, String> = parts
        .into_iter()
        .filter_map(|p| {
            let trimmed = p.trim();
            let eq = trimmed.find('=')?;
            let key = trimmed[..eq].trim().to_string();
            let val = trimmed[eq + 1..].trim().trim_matches('"').to_string();
            if key.is_empty() {
                None
            } else {
                Some((key, val))
            }
        })
        .collect();

    out.client = pairs.get("Client").cloned();
    out.device = pairs.get("Device").cloned();
    out.device_id = pairs.get("DeviceId").cloned();
    out.version = pairs.get("Version").cloned();
    out.token = pairs.get("Token").cloned();
    out
}

/// Extract the auth token clients send. Order: X-Emby-Token, X-MediaBrowser-Token,
/// then the `Token=` pair inside (X-Emby-)Authorization, then `api_key` query.
pub fn extract_token(req: &HttpRequest) -> Option<String> {
    let headers = req.headers();
    for name in ["x-emby-token", "x-mediabrowser-token"] {
        if let Some(v) = headers.get(name) {
            if let Ok(s) = v.to_str() {
                let t = s.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
    }
    for name in ["x-emby-authorization", "authorization"] {
        if let Some(v) = headers.get(name) {
            if let Ok(s) = v.to_str() {
                let parsed = parse_emby_auth_header(s);
                if let Some(t) = parsed.token {
                    if !t.is_empty() {
                        return Some(t);
                    }
                }
            }
        }
    }
    // Some clients pass it as a query param on streaming URLs.
    let query = req.query_string();
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if k.eq_ignore_ascii_case("api_key") || k.eq_ignore_ascii_case("apikey") {
            if !v.is_empty() {
                return Some(urlencoding::decode(v).map(|s| s.into_owned()).unwrap_or_else(|_| v.to_string()));
            }
        }
    }
    None
}

pub async fn token_valid(pool: &Db, token: &str) -> bool {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT user_id FROM jellyfin_tokens WHERE token = ?1",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    row.is_some()
}

pub async fn store_token(
    pool: &Db,
    token: &str,
    user_id: &str,
    auth: &EmbyAuth,
    now: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO jellyfin_tokens (token, user_id, device_id, device_name, client, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(token) DO NOTHING",
    )
    .bind(token)
    .bind(user_id)
    .bind(auth.device_id.as_deref())
    .bind(auth.device.as_deref())
    .bind(auth.client.as_deref())
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    // Try /dev/urandom; fall back to a time+counter hash if unavailable.
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf))
        .is_ok();
    if !ok {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut h = Sha256::new();
        h.update(nanos.to_le_bytes());
        h.update(n.to_le_bytes());
        let digest = h.finalize();
        let take = bytes.min(digest.len());
        buf[..take].copy_from_slice(&digest[..take]);
    }
    hex::encode(buf)
}

/// Looks up or generates a stable server id, formatted as a dashed UUID and
/// persisted in `jellyfin_meta` so it survives restarts. Jellyfin clients
/// built on the Kotlin/Java SDKs require the UUID format here too.
pub async fn ensure_server_id(pool: &Db) -> Result<String> {
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT value FROM jellyfin_meta WHERE key = 'server_id'")
            .fetch_optional(pool)
            .await?;
    if let Some((v,)) = existing {
        // Older versions of smolsonic stored the un-dashed form; promote it
        // to UUID format in place so clients accept us after an upgrade.
        if v.len() == 32 && v.chars().all(|c| c.is_ascii_hexdigit()) {
            let upgraded = super::mapping::guid_dashed(&v);
            sqlx::query("UPDATE jellyfin_meta SET value = ?1 WHERE key = 'server_id'")
                .bind(&upgraded)
                .execute(pool)
                .await?;
            return Ok(upgraded);
        }
        return Ok(v);
    }
    let id = super::mapping::guid_dashed(&random_hex(16));
    sqlx::query(
        "INSERT INTO jellyfin_meta (key, value) VALUES ('server_id', ?1)
         ON CONFLICT(key) DO NOTHING",
    )
    .bind(&id)
    .execute(pool)
    .await?;
    let (val,): (String,) =
        sqlx::query_as("SELECT value FROM jellyfin_meta WHERE key = 'server_id'")
            .fetch_one(pool)
            .await?;
    Ok(val)
}

// ── FromRequest extractor for protected endpoints ─────────────────────────────

pub struct AuthedUser {
    #[allow(dead_code)]
    pub user_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test::TestRequest;
    use sqlx::sqlite::SqlitePoolOptions;

    #[test]
    fn parses_mediabrowser_header() {
        let h = r#"MediaBrowser Client="Finamp", Device="iPhone", DeviceId="abc-123", Version="1.2.3", Token="tok42""#;
        let a = parse_emby_auth_header(h);
        assert_eq!(a.client.as_deref(), Some("Finamp"));
        assert_eq!(a.device.as_deref(), Some("iPhone"));
        assert_eq!(a.device_id.as_deref(), Some("abc-123"));
        assert_eq!(a.version.as_deref(), Some("1.2.3"));
        assert_eq!(a.token.as_deref(), Some("tok42"));
    }

    #[test]
    fn parses_emby_prefix() {
        let h = r#"Emby Client="x", Token="t""#;
        let a = parse_emby_auth_header(h);
        assert_eq!(a.client.as_deref(), Some("x"));
        assert_eq!(a.token.as_deref(), Some("t"));
    }

    #[test]
    fn parses_unquoted_values() {
        let h = "MediaBrowser Client=foo, Token=bar";
        let a = parse_emby_auth_header(h);
        assert_eq!(a.client.as_deref(), Some("foo"));
        assert_eq!(a.token.as_deref(), Some("bar"));
    }

    #[test]
    fn ignores_commas_inside_quoted_values() {
        // Some clients send "Device=\"My iPhone, Pro\"" — the comma is not a separator.
        let h = r#"MediaBrowser Client="x", Device="My iPhone, Pro", Token="t""#;
        let a = parse_emby_auth_header(h);
        assert_eq!(a.device.as_deref(), Some("My iPhone, Pro"));
        assert_eq!(a.token.as_deref(), Some("t"));
    }

    #[test]
    fn extract_token_prefers_x_emby_token_header() {
        let req = TestRequest::default()
            .insert_header(("X-Emby-Token", "header-token"))
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Token="auth-token""#,
            ))
            .to_http_request();
        assert_eq!(extract_token(&req).as_deref(), Some("header-token"));
    }

    #[test]
    fn extract_token_falls_back_to_authorization_header() {
        let req = TestRequest::default()
            .insert_header((
                "Authorization",
                r#"MediaBrowser Token="auth-token""#,
            ))
            .to_http_request();
        assert_eq!(extract_token(&req).as_deref(), Some("auth-token"));
    }

    #[test]
    fn extract_token_falls_back_to_api_key_query() {
        let req = TestRequest::default()
            .uri("/Audio/abc/stream?api_key=qtoken&foo=bar")
            .to_http_request();
        assert_eq!(extract_token(&req).as_deref(), Some("qtoken"));
    }

    #[test]
    fn extract_token_url_decodes_api_key() {
        let req = TestRequest::default()
            .uri("/Audio/abc/stream?api_key=t%20ok")
            .to_http_request();
        assert_eq!(extract_token(&req).as_deref(), Some("t ok"));
    }

    #[test]
    fn extract_token_returns_none_when_absent() {
        let req = TestRequest::default().to_http_request();
        assert!(extract_token(&req).is_none());
    }

    #[test]
    fn random_hex_is_correct_length() {
        let h = random_hex(16);
        assert_eq!(h.len(), 32);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn store_and_validate_token() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE jellyfin_tokens (
                token TEXT PRIMARY KEY, user_id TEXT NOT NULL,
                device_id TEXT, device_name TEXT, client TEXT,
                created_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let auth = EmbyAuth {
            client: Some("test".into()),
            ..Default::default()
        };
        store_token(&pool, "tok", "user1", &auth, "2026-01-01T00:00:00Z")
            .await
            .unwrap();

        assert!(token_valid(&pool, "tok").await);
        assert!(!token_valid(&pool, "other").await);
    }

    #[tokio::test]
    async fn ensure_server_id_is_stable_across_calls() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE jellyfin_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let a = ensure_server_id(&pool).await.unwrap();
        let b = ensure_server_id(&pool).await.unwrap();
        assert_eq!(a, b);
        // Dashed UUID: 32 hex chars + 4 dashes.
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[8], b'-');
    }
}

impl FromRequest for AuthedUser {
    type Error = actix_web::Error;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let token = extract_token(req);
        let state = req.app_data::<web::Data<JellyfinState>>().cloned();
        Box::pin(async move {
            let Some(token) = token else {
                return Err(actix_web::error::ErrorUnauthorized("missing access token"));
            };
            let Some(state) = state else {
                return Err(actix_web::error::ErrorInternalServerError("missing state"));
            };
            if token_valid(&state.pool, &token).await {
                Ok(AuthedUser {
                    user_id: state.user_id.as_str().to_string(),
                })
            } else {
                Err(actix_web::error::ErrorUnauthorized("invalid token"))
            }
        })
    }
}
