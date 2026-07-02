//! Last.fm + MusicBrainz plugins backing the Jellyfin `/…/Similar` rails.
//!
//! Both plugins are **opt-in**: they're constructed only when the operator
//! sets `[lastfm].api_key` or `[musicbrainz].user_agent` in `smolsonic.toml`.
//! When neither is enabled, `SimilarProviders::similar_artist_names` returns
//! an empty list and the `/…/Similar` handlers respond with an empty
//! `ItemsResult` — matching how a stock Jellyfin server behaves with no
//! metadata providers installed.
//!
//! Results are cached in the `similar_artists_cache` sidecar for `CACHE_TTL`
//! so repeat calls (client re-open, rail re-render) don't pound the remote.

use crate::db::Db;
use anyhow::Result;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// TTL for cached similar-artist lists. Long enough to survive a full session
/// of a heavy client without re-fetching; short enough that operators see
/// updates within a week when they refresh their library.
const CACHE_TTL: Duration = Duration::days(7);

/// MB's TOS caps clients at 1 request per second across the whole app. We
/// serialise MB calls behind a mutex + `last_call` timestamp so that even
/// bursty rails stay compliant.
const MUSICBRAINZ_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1100);

pub struct SimilarProviders {
    pub lastfm: Option<LastfmClient>,
    pub musicbrainz: Option<MusicBrainzClient>,
}

impl SimilarProviders {
    pub fn new(
        lastfm: Option<&crate::config::LastfmConfig>,
        musicbrainz: Option<&crate::config::MusicbrainzConfig>,
    ) -> Self {
        Self {
            lastfm: lastfm.map(|c| LastfmClient::new(c.api_key.clone())),
            musicbrainz: musicbrainz.map(|c| MusicBrainzClient::new(c.user_agent.clone())),
        }
    }

    /// True iff at least one plugin is enabled. Handlers use this to
    /// short-circuit to an empty response when neither token is configured.
    pub fn any_enabled(&self) -> bool {
        self.lastfm.is_some() || self.musicbrainz.is_some()
    }

    /// Union of similar-artist names from every enabled provider, deduped by
    /// case-insensitive name. Cached in `similar_artists_cache` keyed by
    /// (seed artist id, provider). A failed provider call is logged and
    /// treated as "no results" so a Last.fm outage doesn't break the rail.
    pub async fn similar_artist_names(
        &self,
        pool: &Db,
        seed_artist_id: &str,
        seed_artist_name: &str,
    ) -> Vec<String> {
        let mut collected: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Some(lf) = &self.lastfm {
            match lf
                .similar_names_cached(pool, seed_artist_id, seed_artist_name)
                .await
            {
                Ok(names) => {
                    for n in names {
                        if seen.insert(n.to_ascii_lowercase()) {
                            collected.push(n);
                        }
                    }
                }
                Err(e) => tracing::warn!("lastfm similar('{seed_artist_name}'): {e}"),
            }
        }
        if let Some(mb) = &self.musicbrainz {
            match mb
                .linked_names_cached(pool, seed_artist_id, seed_artist_name)
                .await
            {
                Ok(names) => {
                    for n in names {
                        if seen.insert(n.to_ascii_lowercase()) {
                            collected.push(n);
                        }
                    }
                }
                Err(e) => tracing::warn!("musicbrainz linked('{seed_artist_name}'): {e}"),
            }
        }
        collected
    }
}

// ── Cache read/write ────────────────────────────────────────────────────────

async fn cache_get(pool: &Db, artist_id: &str, provider: &str) -> Result<Option<Vec<String>>> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT names_json, fetched_at FROM similar_artists_cache
         WHERE artist_id = ?1 AND provider = ?2",
    )
    .bind(artist_id)
    .bind(provider)
    .fetch_optional(pool)
    .await?;
    let Some((json, when)) = row else {
        return Ok(None);
    };
    // Expired → treat as absent (caller re-fetches; upsert overwrites).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&when) {
        if Utc::now().signed_duration_since(dt.with_timezone(&Utc)) > CACHE_TTL {
            return Ok(None);
        }
    }
    let names: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
    Ok(Some(names))
}

async fn cache_put(pool: &Db, artist_id: &str, provider: &str, names: &[String]) -> Result<()> {
    let json = serde_json::to_string(names)?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO similar_artists_cache (artist_id, provider, names_json, fetched_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(artist_id, provider) DO UPDATE SET
            names_json = excluded.names_json,
            fetched_at = excluded.fetched_at",
    )
    .bind(artist_id)
    .bind(provider)
    .bind(json)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

// ── Last.fm ─────────────────────────────────────────────────────────────────

pub struct LastfmClient {
    api_key: String,
    http: reqwest::Client,
}

impl LastfmClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .expect("build reqwest client"),
        }
    }

    async fn similar_names_cached(
        &self,
        pool: &Db,
        seed_artist_id: &str,
        seed_name: &str,
    ) -> Result<Vec<String>> {
        if let Some(cached) = cache_get(pool, seed_artist_id, "lastfm").await? {
            return Ok(cached);
        }
        let names = self.fetch_similar(seed_name).await?;
        cache_put(pool, seed_artist_id, "lastfm", &names).await?;
        Ok(names)
    }

    /// Call `artist.getsimilar` and return the (up to `limit`) names in
    /// match-score order. Uses `?artist=` name lookup since smolsonic
    /// doesn't scan MBIDs into the DB.
    async fn fetch_similar(&self, seed_name: &str) -> Result<Vec<String>> {
        let url = "http://ws.audioscrobbler.com/2.0/";
        let resp: LastfmSimilarResponse = self
            .http
            .get(url)
            .query(&[
                ("method", "artist.getsimilar"),
                ("artist", seed_name),
                ("autocorrect", "1"),
                ("limit", "30"),
                ("api_key", self.api_key.as_str()),
                ("format", "json"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_lastfm_names(&resp))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct LastfmSimilarResponse {
    #[serde(default)]
    similarartists: Option<LastfmSimilarInner>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LastfmSimilarInner {
    #[serde(default)]
    artist: Vec<LastfmSimilarArtist>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LastfmSimilarArtist {
    #[serde(default)]
    name: String,
}

fn parse_lastfm_names(resp: &LastfmSimilarResponse) -> Vec<String> {
    resp.similarartists
        .as_ref()
        .map(|inner| {
            inner
                .artist
                .iter()
                .map(|a| a.name.clone())
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

// ── MusicBrainz ─────────────────────────────────────────────────────────────

pub struct MusicBrainzClient {
    user_agent: String,
    http: reqwest::Client,
    /// Global rate-limiter — MB permits 1 req/s across the whole app.
    lock: Arc<Mutex<Option<std::time::Instant>>>,
}

impl MusicBrainzClient {
    pub fn new(user_agent: String) -> Self {
        Self {
            user_agent,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .expect("build reqwest client"),
            lock: Arc::new(Mutex::new(None)),
        }
    }

    async fn linked_names_cached(
        &self,
        pool: &Db,
        seed_artist_id: &str,
        seed_name: &str,
    ) -> Result<Vec<String>> {
        if let Some(cached) = cache_get(pool, seed_artist_id, "musicbrainz").await? {
            return Ok(cached);
        }
        let names = self.fetch_linked(seed_name).await.unwrap_or_default();
        cache_put(pool, seed_artist_id, "musicbrainz", &names).await?;
        Ok(names)
    }

    async fn wait_for_slot(&self) {
        let mut guard = self.lock.lock().await;
        if let Some(last) = *guard {
            let elapsed = last.elapsed();
            if elapsed < MUSICBRAINZ_MIN_INTERVAL {
                tokio::time::sleep(MUSICBRAINZ_MIN_INTERVAL - elapsed).await;
            }
        }
        *guard = Some(std::time::Instant::now());
    }

    async fn fetch_linked(&self, seed_name: &str) -> Result<Vec<String>> {
        // Step 1: resolve name → MBID via the search index.
        self.wait_for_slot().await;
        let search: MbSearchResponse = self
            .http
            .get("https://musicbrainz.org/ws/2/artist")
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .query(&[
                ("query", format!("artist:\"{seed_name}\"").as_str()),
                ("limit", "1"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(mbid) = search.artists.first().map(|a| a.id.clone()) else {
            return Ok(Vec::new());
        };

        // Step 2: pull artist-artist relationships (band members, collaborations).
        self.wait_for_slot().await;
        let rels: MbArtistResponse = self
            .http
            .get(format!("https://musicbrainz.org/ws/2/artist/{mbid}"))
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .query(&[("inc", "artist-rels"), ("fmt", "json")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_mb_linked(&rels))
    }
}

#[derive(Debug, Deserialize)]
struct MbSearchResponse {
    #[serde(default)]
    artists: Vec<MbSearchArtist>,
}

#[derive(Debug, Deserialize)]
struct MbSearchArtist {
    id: String,
}

#[derive(Debug, Deserialize)]
struct MbArtistResponse {
    #[serde(default, rename = "relations")]
    relations: Vec<MbRelation>,
}

#[derive(Debug, Deserialize)]
struct MbRelation {
    #[serde(default)]
    artist: Option<MbRelationArtist>,
}

#[derive(Debug, Deserialize)]
struct MbRelationArtist {
    #[serde(default)]
    name: String,
}

fn parse_mb_linked(resp: &MbArtistResponse) -> Vec<String> {
    resp.relations
        .iter()
        .filter_map(|r| r.artist.as_ref())
        .map(|a| a.name.clone())
        .filter(|n| !n.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lastfm_similar_response() {
        let payload = r#"{
            "similarartists": {
                "artist": [
                    {"name": "Similar Artist A"},
                    {"name": "Similar Artist B"},
                    {"name": ""}
                ]
            }
        }"#;
        let resp: LastfmSimilarResponse = serde_json::from_str(payload).unwrap();
        let names = parse_lastfm_names(&resp);
        assert_eq!(names, vec!["Similar Artist A", "Similar Artist B"]);
    }

    #[test]
    fn parses_lastfm_missing_similarartists_as_empty() {
        let payload = r#"{"error": 6, "message": "not found"}"#;
        let resp: LastfmSimilarResponse = serde_json::from_str(payload).unwrap();
        assert!(parse_lastfm_names(&resp).is_empty());
    }

    #[test]
    fn parses_musicbrainz_artist_rels() {
        let payload = r#"{
            "relations": [
                {"artist": {"name": "Guest Artist"}},
                {"artist": {"name": "Band Member"}},
                {"work": {"title": "not an artist relation"}}
            ]
        }"#;
        let resp: MbArtistResponse = serde_json::from_str(payload).unwrap();
        let names = parse_mb_linked(&resp);
        assert_eq!(names, vec!["Guest Artist", "Band Member"]);
    }

    #[tokio::test]
    async fn cache_roundtrip_returns_none_when_expired() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE similar_artists_cache (
                artist_id TEXT NOT NULL, provider TEXT NOT NULL,
                names_json TEXT NOT NULL, fetched_at TEXT NOT NULL,
                PRIMARY KEY (artist_id, provider))",
        )
        .execute(&pool)
        .await
        .unwrap();

        cache_put(&pool, "ar-1", "lastfm", &["A".into(), "B".into()])
            .await
            .unwrap();
        let hit = cache_get(&pool, "ar-1", "lastfm").await.unwrap();
        assert_eq!(hit, Some(vec!["A".to_string(), "B".to_string()]));

        // Force the row to look expired.
        let stale = (Utc::now() - Duration::days(30)).to_rfc3339();
        sqlx::query("UPDATE similar_artists_cache SET fetched_at = ?1")
            .bind(&stale)
            .execute(&pool)
            .await
            .unwrap();
        let miss = cache_get(&pool, "ar-1", "lastfm").await.unwrap();
        assert!(miss.is_none());
    }
}
