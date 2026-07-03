//! Optional Typesense search backend for the free-text search endpoints.
//!
//! When `[typesense]` is present in `smolsonic.toml`, `search3` / `search2` /
//! Jellyfin `?searchTerm=` route through Typesense instead of the built-in
//! SQLite FTS5 index. All write paths (scanner + watcher) mirror
//! `songs / albums / artists` inserts, updates, and deletes into three
//! Typesense collections. On any Typesense failure the search layer logs a
//! warning and falls back to FTS5, so a Typesense outage never breaks search.

use crate::config::TypesenseConfig;
use crate::db::Db;
use crate::models::{Album, Artist, Song};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Text collections we manage. Fixed set — every write path knows about all
/// three and reindex walks all three sequentially.
const SONGS: &str = "songs";
const ALBUMS: &str = "albums";
const ARTISTS: &str = "artists";

pub struct TypesenseClient {
    base_url: String,
    api_key: String,
    prefix: String,
    http: reqwest::Client,
}

impl TypesenseClient {
    pub fn new(cfg: &TypesenseConfig) -> Self {
        Self {
            base_url: cfg.url.trim_end_matches('/').to_string(),
            api_key: cfg.api_key.clone(),
            prefix: cfg.collection_prefix.clone(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .expect("build reqwest client"),
        }
    }

    fn collection(&self, kind: &str) -> String {
        format!("{}_{}", self.prefix, kind)
    }

    /// Create the three collections if they don't already exist. Idempotent —
    /// a 409 "already exists" is treated as success.
    pub async fn bootstrap(&self) -> Result<()> {
        self.ensure_collection(
            SONGS,
            &[
                field("title", "string"),
                field("artist", "string"),
                field("album", "string"),
                field_optional("genre", "string"),
                field_optional("year", "int32"),
            ],
        )
        .await?;
        self.ensure_collection(
            ALBUMS,
            &[
                field("title", "string"),
                field("artist", "string"),
                field_optional("year", "int32"),
            ],
        )
        .await?;
        self.ensure_collection(ARTISTS, &[field("name", "string")])
            .await?;
        Ok(())
    }

    async fn ensure_collection(&self, kind: &str, fields: &[serde_json::Value]) -> Result<()> {
        let name = self.collection(kind);
        let url = format!("{}/collections", self.base_url);
        let body = json!({
            "name": name,
            "fields": fields,
            "default_sorting_field": "",
        });
        let resp = self
            .http
            .post(&url)
            .header("X-TYPESENSE-API-KEY", &self.api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("typesense: create collection {name}"))?;
        let status = resp.status();
        if status.is_success() || status.as_u16() == 409 {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        Err(anyhow!(
            "typesense: create collection {name} -> {status}: {text}"
        ))
    }

    // ── Writes ──────────────────────────────────────────────────────────────

    pub async fn upsert_song(&self, song: &Song) -> Result<()> {
        let doc = json!({
            "id": song.id,
            "title": song.title,
            "artist": song.artist,
            "album": song.album,
            "genre": song.genre.clone().unwrap_or_default(),
            "year": song.year.unwrap_or(0),
        });
        self.upsert_doc(SONGS, doc).await
    }

    pub async fn upsert_album(&self, album: &Album) -> Result<()> {
        let doc = json!({
            "id": album.id,
            "title": album.title,
            "artist": album.artist,
            "year": album.year,
        });
        self.upsert_doc(ALBUMS, doc).await
    }

    pub async fn upsert_artist(&self, artist: &Artist) -> Result<()> {
        let doc = json!({
            "id": artist.id,
            "name": artist.name,
        });
        self.upsert_doc(ARTISTS, doc).await
    }

    async fn upsert_doc(&self, kind: &str, doc: serde_json::Value) -> Result<()> {
        let name = self.collection(kind);
        let url = format!(
            "{}/collections/{}/documents?action=upsert",
            self.base_url, name
        );
        let resp = self
            .http
            .post(&url)
            .header("X-TYPESENSE-API-KEY", &self.api_key)
            .json(&doc)
            .send()
            .await
            .with_context(|| format!("typesense: upsert into {name}"))?;
        if resp.status().is_success() {
            return Ok(());
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(anyhow!("typesense: upsert {name} -> {status}: {text}"))
    }

    pub async fn delete_song(&self, id: &str) -> Result<()> {
        self.delete_doc(SONGS, id).await
    }

    pub async fn delete_album(&self, id: &str) -> Result<()> {
        self.delete_doc(ALBUMS, id).await
    }

    pub async fn delete_artist(&self, id: &str) -> Result<()> {
        self.delete_doc(ARTISTS, id).await
    }

    async fn delete_doc(&self, kind: &str, id: &str) -> Result<()> {
        let name = self.collection(kind);
        let url = format!(
            "{}/collections/{}/documents/{}",
            self.base_url,
            name,
            urlencoding::encode(id)
        );
        let resp = self
            .http
            .delete(&url)
            .header("X-TYPESENSE-API-KEY", &self.api_key)
            .send()
            .await
            .with_context(|| format!("typesense: delete from {name}"))?;
        let status = resp.status();
        // 404 is fine: the caller might delete after we already GC'd the id
        // through a prior reconcile.
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        Err(anyhow!("typesense: delete {name} -> {status}: {text}"))
    }

    // ── Searches ────────────────────────────────────────────────────────────

    pub async fn search_song_ids(
        &self,
        term: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<String>> {
        self.search(SONGS, "title,artist,album,genre", term, limit, offset)
            .await
    }

    pub async fn search_album_ids(
        &self,
        term: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<String>> {
        self.search(ALBUMS, "title,artist", term, limit, offset)
            .await
    }

    pub async fn search_artist_ids(
        &self,
        term: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<String>> {
        self.search(ARTISTS, "name", term, limit, offset).await
    }

    async fn search(
        &self,
        kind: &str,
        query_by: &str,
        term: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<String>> {
        let name = self.collection(kind);
        // Typesense pagination uses page/per_page (1-based). Convert
        // offset/limit — offset must be a clean multiple of limit for exact
        // pagination; when it isn't (rare in this app) we round down and
        // clients get slightly overlapping windows, which is fine.
        let per_page = limit.clamp(1, 250);
        let page = if per_page > 0 {
            (offset / per_page) + 1
        } else {
            1
        };
        let url = format!("{}/collections/{}/documents/search", self.base_url, name);
        let resp = self
            .http
            .get(&url)
            .header("X-TYPESENSE-API-KEY", &self.api_key)
            .query(&[
                ("q", term),
                ("query_by", query_by),
                ("per_page", &per_page.to_string()),
                ("page", &page.to_string()),
                ("include_fields", "id"),
            ])
            .send()
            .await
            .with_context(|| format!("typesense: search {name}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("typesense: search {name} -> {status}: {text}"));
        }
        let parsed: SearchResponse = resp
            .json()
            .await
            .with_context(|| format!("typesense: parse search response for {name}"))?;
        Ok(parsed
            .hits
            .into_iter()
            .filter_map(|h| h.document.id)
            .collect())
    }

    // ── Startup reindex ─────────────────────────────────────────────────────

    /// True iff the songs collection currently has zero documents. Used to
    /// decide whether to run the initial reindex on startup.
    pub async fn songs_empty(&self) -> Result<bool> {
        let name = self.collection(SONGS);
        let url = format!("{}/collections/{}", self.base_url, name);
        let resp = self
            .http
            .get(&url)
            .header("X-TYPESENSE-API-KEY", &self.api_key)
            .send()
            .await
            .with_context(|| format!("typesense: describe {name}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("typesense: describe {name} -> {status}: {text}"));
        }
        let info: CollectionInfo = resp.json().await?;
        Ok(info.num_documents == 0)
    }

    /// Walk the SQLite tables and bulk-import every artist / album / song
    /// into Typesense. Called from `main` at startup when the songs
    /// collection reports 0 documents, so a fresh Typesense node self-heals
    /// without operator intervention.
    pub async fn reindex_from_db(&self, pool: &Db) -> Result<()> {
        let artists: Vec<Artist> =
            sqlx::query_as("SELECT id, name FROM artists ORDER BY name COLLATE NOCASE")
                .fetch_all(pool)
                .await?;
        let artist_lines: Vec<String> = artists
            .iter()
            .filter_map(|a| serde_json::to_string(&json!({ "id": a.id, "name": a.name })).ok())
            .collect();
        self.import_jsonl(ARTISTS, &artist_lines).await?;

        let albums: Vec<Album> = sqlx::query_as(
            "SELECT id, title, artist, artist_id, year, cover_art FROM albums
             ORDER BY title COLLATE NOCASE",
        )
        .fetch_all(pool)
        .await?;
        let album_lines: Vec<String> = albums
            .iter()
            .filter_map(|a| {
                serde_json::to_string(&json!({
                    "id": a.id,
                    "title": a.title,
                    "artist": a.artist,
                    "year": a.year,
                }))
                .ok()
            })
            .collect();
        self.import_jsonl(ALBUMS, &album_lines).await?;

        let songs: Vec<Song> = sqlx::query_as(
            "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                    disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
             FROM songs ORDER BY title COLLATE NOCASE",
        )
        .fetch_all(pool)
        .await?;
        let song_lines: Vec<String> = songs
            .iter()
            .filter_map(|s| {
                serde_json::to_string(&json!({
                    "id": s.id,
                    "title": s.title,
                    "artist": s.artist,
                    "album": s.album,
                    "genre": s.genre.clone().unwrap_or_default(),
                    "year": s.year.unwrap_or(0),
                }))
                .ok()
            })
            .collect();
        self.import_jsonl(SONGS, &song_lines).await?;

        tracing::info!(
            "typesense: reindexed {} artists, {} albums, {} songs",
            artists.len(),
            albums.len(),
            songs.len()
        );
        Ok(())
    }

    /// Typesense bulk-import wire format is newline-delimited JSON, one
    /// document per line. Empty payload is a no-op.
    async fn import_jsonl(&self, kind: &str, lines: &[String]) -> Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let name = self.collection(kind);
        let url = format!(
            "{}/collections/{}/documents/import?action=upsert",
            self.base_url, name
        );
        let body = lines.join("\n");
        let resp = self
            .http
            .post(&url)
            .header("X-TYPESENSE-API-KEY", &self.api_key)
            .header("Content-Type", "text/plain")
            .body(body)
            .send()
            .await
            .with_context(|| format!("typesense: import {name}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("typesense: import {name} -> {status}: {text}"));
        }
        Ok(())
    }
}

// ── Wire shapes ──────────────────────────────────────────────────────────────

fn field(name: &str, ty: &str) -> serde_json::Value {
    json!({ "name": name, "type": ty })
}

fn field_optional(name: &str, ty: &str) -> serde_json::Value {
    json!({ "name": name, "type": ty, "optional": true })
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    document: HitDoc,
}

#[derive(Deserialize)]
struct HitDoc {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct CollectionInfo {
    #[serde(default)]
    num_documents: i64,
}

// The Serialize derives are here so the type is usable with `serde_json`
// without an extra roundtrip when we add richer document payloads later.
#[allow(dead_code)]
#[derive(Serialize)]
struct SongDoc<'a> {
    id: &'a str,
    title: &'a str,
    artist: &'a str,
    album: &'a str,
    genre: &'a str,
    year: i64,
}
