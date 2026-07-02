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

    /// Look up cover-art URLs for an album via `album.getInfo`. Note that
    /// Last.fm's response usually contains empty image strings post-2018
    /// (licensing), but we still call it so we surface whatever it returns
    /// alongside MusicBrainz. Cached under the "lastfm-album-image" tag.
    pub async fn album_image_urls_cached(
        &self,
        pool: &Db,
        album_native_id: &str,
        artist: &str,
        album: &str,
    ) -> Result<Vec<String>> {
        if let Some(cached) = cache_get(pool, album_native_id, "lastfm-album-image").await? {
            return Ok(cached);
        }
        let urls = self.fetch_album_images(artist, album).await?;
        cache_put(pool, album_native_id, "lastfm-album-image", &urls).await?;
        Ok(urls)
    }

    async fn fetch_album_images(&self, artist: &str, album: &str) -> Result<Vec<String>> {
        let resp: LastfmAlbumInfoResponse = self
            .http
            .get("http://ws.audioscrobbler.com/2.0/")
            .query(&[
                ("method", "album.getinfo"),
                ("artist", artist),
                ("album", album),
                ("autocorrect", "1"),
                ("api_key", self.api_key.as_str()),
                ("format", "json"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_lastfm_album_images(&resp))
    }

    /// `artist.getInfo` → image URLs. Same 2018-licensing caveat as
    /// `album.getInfo` — expect mostly empty responses.
    pub async fn artist_image_urls_cached(
        &self,
        pool: &Db,
        artist_native_id: &str,
        artist: &str,
    ) -> Result<Vec<String>> {
        if let Some(cached) = cache_get(pool, artist_native_id, "lastfm-artist-image").await? {
            return Ok(cached);
        }
        let urls = self.fetch_artist_images(artist).await?;
        cache_put(pool, artist_native_id, "lastfm-artist-image", &urls).await?;
        Ok(urls)
    }

    async fn fetch_artist_images(&self, artist: &str) -> Result<Vec<String>> {
        let resp: LastfmArtistInfoResponse = self
            .http
            .get("http://ws.audioscrobbler.com/2.0/")
            .query(&[
                ("method", "artist.getinfo"),
                ("artist", artist),
                ("autocorrect", "1"),
                ("api_key", self.api_key.as_str()),
                ("format", "json"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_lastfm_artist_images(&resp))
    }

    /// `album.getInfo` → wiki summary + top tags + Last.fm URL for an album.
    /// Same shape and cache semantics as `artist_info_cached` but stashed
    /// in the sibling `lastfm_album_info` sidecar. Populates album detail
    /// `Overview` / `Tags` / `ExternalUrls`.
    pub async fn album_info_cached(
        &self,
        pool: &Db,
        album_native_id: &str,
        artist: &str,
        album: &str,
    ) -> Result<AlbumInfo> {
        if let Some(cached) = album_info_cache_get(pool, album_native_id).await? {
            return Ok(cached);
        }
        let info = self
            .fetch_album_info(artist, album)
            .await
            .unwrap_or_default();
        album_info_cache_put(pool, album_native_id, &info).await?;
        Ok(info)
    }

    async fn fetch_album_info(&self, artist: &str, album: &str) -> Result<AlbumInfo> {
        let resp: LastfmAlbumInfoResponse = self
            .http
            .get("http://ws.audioscrobbler.com/2.0/")
            .query(&[
                ("method", "album.getinfo"),
                ("artist", artist),
                ("album", album),
                ("autocorrect", "1"),
                ("api_key", self.api_key.as_str()),
                ("format", "json"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_lastfm_album_info(&resp))
    }

    /// `artist.getInfo` → biography summary + top tags + Last.fm URL, all
    /// stashed in `lastfm_artist_info` with a 7-day TTL. Populates the
    /// `Overview`, `Tags`, and `ExternalUrls` fields of the artist detail
    /// BaseItemDto.
    pub async fn artist_info_cached(
        &self,
        pool: &Db,
        artist_native_id: &str,
        artist_name: &str,
    ) -> Result<ArtistInfo> {
        if let Some(cached) = artist_info_cache_get(pool, artist_native_id).await? {
            return Ok(cached);
        }
        let info = self
            .fetch_artist_info(artist_name)
            .await
            .unwrap_or_default();
        artist_info_cache_put(pool, artist_native_id, &info).await?;
        Ok(info)
    }

    async fn fetch_artist_info(&self, artist: &str) -> Result<ArtistInfo> {
        let resp: LastfmArtistInfoResponse = self
            .http
            .get("http://ws.audioscrobbler.com/2.0/")
            .query(&[
                ("method", "artist.getinfo"),
                ("artist", artist),
                ("autocorrect", "1"),
                ("api_key", self.api_key.as_str()),
                ("format", "json"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_lastfm_artist_info(&resp))
    }
}

/// Biography + top-tag payload sourced from Last.fm `artist.getInfo`.
/// Every field is optional — Last.fm returns partial data for many
/// artists and we surface whatever we get.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtistInfo {
    pub bio: Option<String>,
    pub tags: Vec<String>,
    pub url: Option<String>,
}

async fn artist_info_cache_get(pool: &Db, artist_id: &str) -> Result<Option<ArtistInfo>> {
    let row: Option<(Option<String>, String, Option<String>, String)> = sqlx::query_as(
        "SELECT bio, tags_json, url, fetched_at FROM lastfm_artist_info WHERE artist_id = ?1",
    )
    .bind(artist_id)
    .fetch_optional(pool)
    .await?;
    let Some((bio, tags_json, url, fetched_at)) = row else {
        return Ok(None);
    };
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&fetched_at) {
        if Utc::now().signed_duration_since(dt.with_timezone(&Utc)) > CACHE_TTL {
            return Ok(None);
        }
    }
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(Some(ArtistInfo { bio, tags, url }))
}

async fn artist_info_cache_put(pool: &Db, artist_id: &str, info: &ArtistInfo) -> Result<()> {
    let tags_json = serde_json::to_string(&info.tags)?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO lastfm_artist_info (artist_id, bio, tags_json, url, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(artist_id) DO UPDATE SET
            bio = excluded.bio,
            tags_json = excluded.tags_json,
            url = excluded.url,
            fetched_at = excluded.fetched_at",
    )
    .bind(artist_id)
    .bind(info.bio.as_deref())
    .bind(tags_json)
    .bind(info.url.as_deref())
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Album-side twin of `ArtistInfo`. Populated from Last.fm `album.getInfo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlbumInfo {
    pub bio: Option<String>,
    pub tags: Vec<String>,
    pub url: Option<String>,
}

async fn album_info_cache_get(pool: &Db, album_id: &str) -> Result<Option<AlbumInfo>> {
    let row: Option<(Option<String>, String, Option<String>, String)> = sqlx::query_as(
        "SELECT bio, tags_json, url, fetched_at FROM lastfm_album_info WHERE album_id = ?1",
    )
    .bind(album_id)
    .fetch_optional(pool)
    .await?;
    let Some((bio, tags_json, url, fetched_at)) = row else {
        return Ok(None);
    };
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&fetched_at) {
        if Utc::now().signed_duration_since(dt.with_timezone(&Utc)) > CACHE_TTL {
            return Ok(None);
        }
    }
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(Some(AlbumInfo { bio, tags, url }))
}

async fn album_info_cache_put(pool: &Db, album_id: &str, info: &AlbumInfo) -> Result<()> {
    let tags_json = serde_json::to_string(&info.tags)?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO lastfm_album_info (album_id, bio, tags_json, url, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(album_id) DO UPDATE SET
            bio = excluded.bio,
            tags_json = excluded.tags_json,
            url = excluded.url,
            fetched_at = excluded.fetched_at",
    )
    .bind(album_id)
    .bind(info.bio.as_deref())
    .bind(tags_json)
    .bind(info.url.as_deref())
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct LastfmAlbumInfoResponse {
    #[serde(default)]
    album: Option<LastfmAlbumInfo>,
}

#[derive(Debug, Deserialize)]
struct LastfmAlbumInfo {
    #[serde(default, rename = "image")]
    images: Vec<LastfmImage>,
    /// Album `wiki` mirrors artist `bio` in shape but the field name differs
    /// (`wiki` for releases, `bio` for artists).
    #[serde(default)]
    wiki: Option<LastfmArtistBio>,
    #[serde(default)]
    tags: Option<LastfmArtistTags>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LastfmArtistInfoResponse {
    #[serde(default)]
    artist: Option<LastfmArtistInfo>,
}

#[derive(Debug, Deserialize)]
struct LastfmArtistInfo {
    #[serde(default, rename = "image")]
    images: Vec<LastfmImage>,
    #[serde(default)]
    bio: Option<LastfmArtistBio>,
    #[serde(default)]
    tags: Option<LastfmArtistTags>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LastfmArtistBio {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    #[allow(dead_code)] // Full-length HTML; we surface `summary` in the DTO instead.
    content: String,
}

#[derive(Debug, Deserialize)]
struct LastfmArtistTags {
    #[serde(default)]
    tag: Vec<LastfmArtistTag>,
}

#[derive(Debug, Deserialize)]
struct LastfmArtistTag {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct LastfmImage {
    #[serde(default, rename = "#text")]
    url: String,
    #[serde(default)]
    #[allow(dead_code)] // useful for callers that want to pick a preferred size
    size: String,
}

fn parse_lastfm_album_images(resp: &LastfmAlbumInfoResponse) -> Vec<String> {
    resp.album
        .as_ref()
        .map(|a| {
            a.images
                .iter()
                .map(|i| i.url.clone())
                .filter(|u| !u.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_lastfm_artist_images(resp: &LastfmArtistInfoResponse) -> Vec<String> {
    resp.artist
        .as_ref()
        .map(|a| {
            a.images
                .iter()
                .map(|i| i.url.clone())
                .filter(|u| !u.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_lastfm_artist_info(resp: &LastfmArtistInfoResponse) -> ArtistInfo {
    let Some(a) = resp.artist.as_ref() else {
        return ArtistInfo::default();
    };
    let bio = a
        .bio
        .as_ref()
        .map(|b| b.summary.trim().to_string())
        .filter(|s| !s.is_empty());
    let tags = a
        .tags
        .as_ref()
        .map(|t| {
            t.tag
                .iter()
                .map(|t| t.name.clone())
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let url = a.url.clone().filter(|u| !u.is_empty());
    ArtistInfo { bio, tags, url }
}

fn parse_lastfm_album_info(resp: &LastfmAlbumInfoResponse) -> AlbumInfo {
    let Some(a) = resp.album.as_ref() else {
        return AlbumInfo::default();
    };
    let bio = a
        .wiki
        .as_ref()
        .map(|w| w.summary.trim().to_string())
        .filter(|s| !s.is_empty());
    let tags = a
        .tags
        .as_ref()
        .map(|t| {
            t.tag
                .iter()
                .map(|t| t.name.clone())
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let url = a.url.clone().filter(|u| !u.is_empty());
    AlbumInfo { bio, tags, url }
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

    /// Cover Art Archive lookup for an album. Two-step: search for a
    /// release MBID by artist+album name, then `GET
    /// https://coverartarchive.org/release/{mbid}` for the CAA JSON.
    /// Cached under "mb-album-image".
    pub async fn album_image_urls_cached(
        &self,
        pool: &Db,
        album_native_id: &str,
        artist: &str,
        album: &str,
    ) -> Result<Vec<String>> {
        if let Some(cached) = cache_get(pool, album_native_id, "mb-album-image").await? {
            return Ok(cached);
        }
        let urls = self
            .fetch_coverart_urls(artist, album)
            .await
            .unwrap_or_default();
        cache_put(pool, album_native_id, "mb-album-image", &urls).await?;
        Ok(urls)
    }

    async fn fetch_coverart_urls(&self, artist: &str, album: &str) -> Result<Vec<String>> {
        self.wait_for_slot().await;
        let search: MbReleaseSearchResponse = self
            .http
            .get("https://musicbrainz.org/ws/2/release")
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .query(&[
                (
                    "query",
                    format!("release:\"{album}\" AND artist:\"{artist}\"").as_str(),
                ),
                ("limit", "1"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(mbid) = search.releases.first().map(|r| r.id.clone()) else {
            return Ok(Vec::new());
        };

        // Cover Art Archive is a separate host (no MB rate limiter needed),
        // but we still keep the interval as a courtesy — it's the same
        // upstream infrastructure.
        self.wait_for_slot().await;
        let caa: CoverArtArchiveResponse = self
            .http
            .get(format!("https://coverartarchive.org/release/{mbid}"))
            .header("User-Agent", &self.user_agent)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_caa_urls(&caa))
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

#[derive(Debug, Deserialize)]
struct MbReleaseSearchResponse {
    #[serde(default)]
    releases: Vec<MbRelease>,
}

#[derive(Debug, Deserialize)]
struct MbRelease {
    id: String,
}

#[derive(Debug, Deserialize)]
struct CoverArtArchiveResponse {
    #[serde(default)]
    images: Vec<CoverArtImage>,
}

#[derive(Debug, Deserialize)]
struct CoverArtImage {
    #[serde(default)]
    image: String,
    #[serde(default)]
    thumbnails: Option<CoverArtThumbnails>,
    #[serde(default)]
    front: bool,
    #[serde(default)]
    approved: bool,
}

#[derive(Debug, Deserialize)]
struct CoverArtThumbnails {
    #[serde(default, rename = "500")]
    #[allow(dead_code)]
    medium: Option<String>,
    #[serde(default, rename = "1200")]
    #[allow(dead_code)]
    large: Option<String>,
}

fn parse_caa_urls(resp: &CoverArtArchiveResponse) -> Vec<String> {
    // Prefer approved front covers; fall back to any front cover; fall back
    // to any image. Emit the full-size `image` URL — CAA serves a redirect
    // to the actual bytes when downloaded.
    let mut out: Vec<String> = Vec::new();
    for i in &resp.images {
        if !i.approved || !i.front {
            continue;
        }
        if !i.image.is_empty() {
            out.push(i.image.clone());
        }
    }
    if out.is_empty() {
        for i in &resp.images {
            if !i.front {
                continue;
            }
            if !i.image.is_empty() {
                out.push(i.image.clone());
            }
        }
    }
    if out.is_empty() {
        for i in &resp.images {
            if !i.image.is_empty() {
                out.push(i.image.clone());
            }
        }
    }
    // Silence unused-field warning on CoverArtThumbnails when out already
    // has content — keep the type available for future callers.
    let _ = resp
        .images
        .iter()
        .filter_map(|i| i.thumbnails.as_ref())
        .count();
    out
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

    #[test]
    fn parses_lastfm_album_getinfo_images() {
        // r##"..."## because the JSON payload contains `"#text"` — a naked
        // `r#"..."#` would terminate at the first inner `"#`.
        let payload = r##"{
            "album": {
                "image": [
                    {"#text": "https://example/small.png", "size": "small"},
                    {"#text": "https://example/large.png", "size": "large"},
                    {"#text": "", "size": "mega"}
                ]
            }
        }"##;
        let resp: LastfmAlbumInfoResponse = serde_json::from_str(payload).unwrap();
        let urls = parse_lastfm_album_images(&resp);
        assert_eq!(
            urls,
            vec!["https://example/small.png", "https://example/large.png"]
        );
    }

    #[test]
    fn parses_caa_prefers_approved_front_covers() {
        let payload = r#"{
            "images": [
                {"image": "https://caa/front-approved.jpg", "front": true, "approved": true},
                {"image": "https://caa/front-pending.jpg", "front": true, "approved": false},
                {"image": "https://caa/back.jpg", "front": false, "approved": true}
            ]
        }"#;
        let resp: CoverArtArchiveResponse = serde_json::from_str(payload).unwrap();
        let urls = parse_caa_urls(&resp);
        assert_eq!(urls, vec!["https://caa/front-approved.jpg"]);
    }

    #[test]
    fn parses_lastfm_artist_info_bio_tags_url() {
        let payload = r#"{
            "artist": {
                "url": "https://www.last.fm/music/Some+Artist",
                "bio": {
                    "summary": "  Some Artist is a band from …  <a href=\"…\">Read more</a> ",
                    "content": "long full bio HTML here"
                },
                "tags": {
                    "tag": [
                        {"name": "indie rock", "url": "…"},
                        {"name": "post-punk", "url": "…"},
                        {"name": "", "url": "…"}
                    ]
                }
            }
        }"#;
        let resp: LastfmArtistInfoResponse = serde_json::from_str(payload).unwrap();
        let info = parse_lastfm_artist_info(&resp);
        assert!(info
            .bio
            .as_ref()
            .unwrap()
            .starts_with("Some Artist is a band"));
        assert_eq!(info.tags, vec!["indie rock", "post-punk"]);
        assert_eq!(
            info.url.as_deref(),
            Some("https://www.last.fm/music/Some+Artist")
        );
    }

    #[test]
    fn parses_lastfm_album_info_wiki_tags_url() {
        let payload = r#"{
            "album": {
                "url": "https://www.last.fm/music/Artist/_/Album",
                "wiki": {
                    "summary": "  Album is a 2020 studio release …  <a href=\"…\">Read more</a> ",
                    "content": "long full wiki HTML"
                },
                "tags": {
                    "tag": [
                        {"name": "indie"},
                        {"name": "dream pop"}
                    ]
                }
            }
        }"#;
        let resp: LastfmAlbumInfoResponse = serde_json::from_str(payload).unwrap();
        let info = parse_lastfm_album_info(&resp);
        assert!(info.bio.as_ref().unwrap().starts_with("Album is a 2020"));
        assert_eq!(info.tags, vec!["indie", "dream pop"]);
        assert_eq!(
            info.url.as_deref(),
            Some("https://www.last.fm/music/Artist/_/Album")
        );
    }

    #[test]
    fn parses_lastfm_album_info_missing_fields_gives_default() {
        let payload = r#"{"album": {}}"#;
        let resp: LastfmAlbumInfoResponse = serde_json::from_str(payload).unwrap();
        let info = parse_lastfm_album_info(&resp);
        assert!(info.bio.is_none());
        assert!(info.tags.is_empty());
        assert!(info.url.is_none());
    }

    #[test]
    fn parses_lastfm_artist_info_missing_fields_gives_default() {
        let payload = r#"{"artist": {}}"#;
        let resp: LastfmArtistInfoResponse = serde_json::from_str(payload).unwrap();
        let info = parse_lastfm_artist_info(&resp);
        assert!(info.bio.is_none());
        assert!(info.tags.is_empty());
        assert!(info.url.is_none());
    }

    #[test]
    fn parses_caa_falls_back_when_no_approved_front() {
        let payload = r#"{
            "images": [
                {"image": "https://caa/pending.jpg", "front": true, "approved": false},
                {"image": "https://caa/back.jpg", "front": false, "approved": true}
            ]
        }"#;
        let resp: CoverArtArchiveResponse = serde_json::from_str(payload).unwrap();
        let urls = parse_caa_urls(&resp);
        assert_eq!(urls, vec!["https://caa/pending.jpg"]);
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
