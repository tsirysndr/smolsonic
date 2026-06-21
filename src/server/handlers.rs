use crate::cli;
use crate::models::{Album, Artist, Song};
use crate::scanner;
use actix_web::{web, HttpRequest, HttpResponse};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use super::{auth, repo, response, SubsonicState};

// ── Root index ────────────────────────────────────────────────────────────────

pub async fn index() -> HttpResponse {
    let body = format!(
        r#"{banner}
  smolsonic v{version}
  a tiny Subsonic-compatible music server

Supported endpoints
  System
    GET  /rest/ping             auth check
    GET  /rest/getUser          single-user response
    GET  /rest/getMusicFolders  one folder
    GET  /rest/getScanStatus    library scan progress
    GET  /rest/startScan        trigger a library rescan

  Library (ID3 tag browsing)
    GET  /rest/getArtists       alphabetical artist index
    GET  /rest/getArtist        ?id=ar-…   albums for an artist
    GET  /rest/getAlbum         ?id=al-…   songs for an album
    GET  /rest/getSong          ?id=so-…   single song lookup
    GET  /rest/getAlbumList2    ?type=alphabeticalByName|alphabeticalByArtist|newest|random

  Playback
    GET  /rest/stream           ?id=so-…   raw audio, Range supported
    GET  /rest/download         ?id=so-…   alias for /rest/stream
    GET  /rest/getCoverArt      ?id=al-…|ar-…|so-…   cached album art

  Search
    GET  /rest/search3          ?query=…&artistCount=&albumCount=&songCount=

Auth (all /rest/* endpoints)
  Token:     u=<user>&t=md5(password+salt)&s=<salt>
  Plaintext: u=<user>&p=<password>      or  p=enc:<hex>

Every endpoint also accepts the `.view` suffix and POST.
Responses are JSON with the Subsonic envelope: {{ "subsonic-response": … }}
"#,
        banner = cli::BANNER,
        version = env!("CARGO_PKG_VERSION"),
    );
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body(body)
}

// ── Common query params ──────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct CommonParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct IdParam {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub id: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct SearchParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub query: Option<String>,
    #[serde(rename = "artistCount")]
    pub artist_count: Option<i64>,
    #[serde(rename = "albumCount")]
    pub album_count: Option<i64>,
    #[serde(rename = "songCount")]
    pub song_count: Option<i64>,
    #[serde(rename = "artistOffset")]
    pub artist_offset: Option<i64>,
    #[serde(rename = "albumOffset")]
    pub album_offset: Option<i64>,
    #[serde(rename = "songOffset")]
    pub song_offset: Option<i64>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct AlbumListParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    #[serde(rename = "type")]
    pub list_type: Option<String>,
    pub size: Option<i64>,
    pub offset: Option<i64>,
}

// ── Auth ──────────────────────────────────────────────────────────────────────

fn require_auth(state: &SubsonicState, q: &CommonLike) -> Option<HttpResponse> {
    if !auth::check(
        &state.username,
        &state.password,
        q.u.as_deref(),
        q.p.as_deref(),
        q.t.as_deref(),
        q.s.as_deref(),
    ) {
        return Some(response::error_json(40, "Wrong username or password"));
    }
    None
}

struct CommonLike<'a> {
    u: Option<&'a str>,
    p: Option<&'a str>,
    t: Option<&'a str>,
    s: Option<&'a str>,
}

impl<'a> CommonLike<'a> {
    fn from_common(c: &'a CommonParams) -> Self {
        Self {
            u: c.u.as_deref(),
            p: c.p.as_deref(),
            t: c.t.as_deref(),
            s: c.s.as_deref(),
        }
    }
    fn from_id(c: &'a IdParam) -> Self {
        Self {
            u: c.u.as_deref(),
            p: c.p.as_deref(),
            t: c.t.as_deref(),
            s: c.s.as_deref(),
        }
    }
    fn from_search(c: &'a SearchParams) -> Self {
        Self {
            u: c.u.as_deref(),
            p: c.p.as_deref(),
            t: c.t.as_deref(),
            s: c.s.as_deref(),
        }
    }
    fn from_album_list(c: &'a AlbumListParams) -> Self {
        Self {
            u: c.u.as_deref(),
            p: c.p.as_deref(),
            t: c.t.as_deref(),
            s: c.s.as_deref(),
        }
    }
}

// ── Mappers ───────────────────────────────────────────────────────────────────

fn song_to_child(s: &Song) -> Value {
    json!({
        "id": s.id,
        "parent": s.album_id,
        "isDir": false,
        "title": s.title,
        "album": s.album,
        "artist": s.artist,
        "track": s.track_number,
        "year": s.year,
        "genre": s.genre,
        "coverArt": s.album_id,
        "size": s.filesize,
        "contentType": s.content_type,
        "suffix": s.suffix,
        "duration": s.duration_ms / 1000,
        "bitRate": s.bitrate,
        "path": s.path,
        "isVideo": false,
        "discNumber": s.disc_number,
        "albumId": s.album_id,
        "artistId": s.artist_id,
        "type": "music",
    })
}

fn album_to_child(a: &Album, song_count: i64, duration_s: i64) -> Value {
    json!({
        "id": a.id,
        "name": a.title,
        "title": a.title,
        "artist": a.artist,
        "artistId": a.artist_id,
        "songCount": song_count,
        "duration": duration_s,
        "year": if a.year > 0 { json!(a.year) } else { Value::Null },
        "coverArt": a.id,
        "created": "2020-01-01T00:00:00Z",
    })
}

fn artist_to_json(a: &Artist, album_count: i64) -> Value {
    json!({
        "id": a.id,
        "name": a.name,
        "albumCount": album_count,
        "coverArt": a.id,
    })
}

fn build_artist_index(artists: &[Artist], counts: &HashMap<String, i64>) -> Vec<Value> {
    let mut groups: HashMap<String, Vec<Value>> = HashMap::new();
    for a in artists {
        let first = a
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().next().unwrap_or(c))
            .unwrap_or('#');
        let key = if first.is_alphabetic() {
            first.to_string()
        } else {
            "#".to_string()
        };
        let count = counts.get(&a.id).copied().unwrap_or(0);
        groups
            .entry(key)
            .or_default()
            .push(artist_to_json(a, count));
    }
    let mut keys: Vec<String> = groups.keys().cloned().collect();
    keys.sort();
    keys.into_iter()
        .map(|k| json!({ "name": k, "artist": groups[&k] }))
        .collect()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn ping(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    response::ok_json(json!({}))
}

pub async fn get_user(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    response::ok_json(json!({
        "user": {
            "username": state.username.as_str(),
            "email": "",
            "scrobblingEnabled": false,
            "adminRole": true,
            "settingsRole": true,
            "downloadRole": true,
            "uploadRole": false,
            "playlistRole": true,
            "coverArtRole": true,
            "commentRole": false,
            "podcastRole": false,
            "streamRole": true,
            "jukeboxRole": false,
            "shareRole": false,
            "folder": [1],
        }
    }))
}

pub async fn get_music_folders(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    response::ok_json(json!({
        "musicFolders": {
            "musicFolder": [
                { "id": 1, "name": "Music" }
            ]
        }
    }))
}

pub async fn get_artists(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    let artists = match repo::all_artists(&state.pool).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("getArtists: {e}");
            return response::error_json(0, "database error");
        }
    };
    let counts = repo::album_counts_by_artist(&state.pool)
        .await
        .unwrap_or_default();
    let index = build_artist_index(&artists, &counts);
    response::ok_json(json!({
        "artists": {
            "ignoredArticles": "The An A Die Das Ein",
            "index": index,
        }
    }))
}

pub async fn get_artist(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    let Some(id) = q.id.as_deref() else {
        return response::error_json(10, "Required parameter is missing: id");
    };
    let artist = match repo::find_artist(&state.pool, id).await {
        Ok(Some(a)) => a,
        Ok(None) => return response::error_json(70, "Artist not found"),
        Err(e) => {
            tracing::error!("getArtist: {e}");
            return response::error_json(0, "database error");
        }
    };
    let albums = repo::albums_by_artist(&state.pool, id).await.unwrap_or_default();
    let mut album_jsons = Vec::with_capacity(albums.len());
    for a in &albums {
        let count = repo::song_count_for_album(&state.pool, &a.id).await.unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id).await.unwrap_or(0);
        album_jsons.push(album_to_child(a, count, dur));
    }
    response::ok_json(json!({
        "artist": {
            "id": artist.id,
            "name": artist.name,
            "albumCount": album_jsons.len(),
            "coverArt": artist.id,
            "album": album_jsons,
        }
    }))
}

pub async fn get_album(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    let Some(id) = q.id.as_deref() else {
        return response::error_json(10, "Required parameter is missing: id");
    };
    let album = match repo::find_album(&state.pool, id).await {
        Ok(Some(a)) => a,
        Ok(None) => return response::error_json(70, "Album not found"),
        Err(e) => {
            tracing::error!("getAlbum: {e}");
            return response::error_json(0, "database error");
        }
    };
    let songs = repo::songs_by_album(&state.pool, id).await.unwrap_or_default();
    let total_dur = songs.iter().map(|s| s.duration_ms / 1000).sum::<i64>();
    let song_jsons: Vec<Value> = songs.iter().map(song_to_child).collect();
    response::ok_json(json!({
        "album": {
            "id": album.id,
            "name": album.title,
            "title": album.title,
            "artist": album.artist,
            "artistId": album.artist_id,
            "coverArt": album.id,
            "songCount": song_jsons.len(),
            "duration": total_dur,
            "year": if album.year > 0 { json!(album.year) } else { Value::Null },
            "song": song_jsons,
        }
    }))
}

pub async fn get_song(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    let Some(id) = q.id.as_deref() else {
        return response::error_json(10, "Required parameter is missing: id");
    };
    let song = match repo::find_song(&state.pool, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return response::error_json(70, "Song not found"),
        Err(e) => {
            tracing::error!("getSong: {e}");
            return response::error_json(0, "database error");
        }
    };
    response::ok_json(json!({ "song": song_to_child(&song) }))
}

pub async fn get_album_list2(
    state: web::Data<SubsonicState>,
    query: web::Query<AlbumListParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_album_list(&q)) {
        return r;
    }
    let list_type = q.list_type.as_deref().unwrap_or("alphabeticalByName");
    let size = q.size.unwrap_or(10).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let albums = repo::albums_paginated(&state.pool, list_type, size, offset)
        .await
        .unwrap_or_default();
    let mut album_jsons = Vec::with_capacity(albums.len());
    for a in &albums {
        let count = repo::song_count_for_album(&state.pool, &a.id).await.unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id).await.unwrap_or(0);
        album_jsons.push(album_to_child(a, count, dur));
    }
    response::ok_json(json!({
        "albumList2": { "album": album_jsons }
    }))
}

pub async fn search3(
    state: web::Data<SubsonicState>,
    query: web::Query<SearchParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_search(&q)) {
        return r;
    }
    let term = q.query.unwrap_or_default();
    let artist_limit = q.artist_count.unwrap_or(20);
    let album_limit = q.album_count.unwrap_or(20);
    let song_limit = q.song_count.unwrap_or(20);
    let artist_offset = q.artist_offset.unwrap_or(0);
    let album_offset = q.album_offset.unwrap_or(0);
    let song_offset = q.song_offset.unwrap_or(0);

    let artists = repo::search_artists(&state.pool, &term, artist_limit, artist_offset)
        .await
        .unwrap_or_default();
    let albums = repo::search_albums(&state.pool, &term, album_limit, album_offset)
        .await
        .unwrap_or_default();
    let songs = repo::search_songs(&state.pool, &term, song_limit, song_offset)
        .await
        .unwrap_or_default();

    let artist_jsons: Vec<Value> = artists.iter().map(|a| artist_to_json(a, 0)).collect();
    let mut album_jsons = Vec::with_capacity(albums.len());
    for a in &albums {
        let count = repo::song_count_for_album(&state.pool, &a.id).await.unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id).await.unwrap_or(0);
        album_jsons.push(album_to_child(a, count, dur));
    }
    let song_jsons: Vec<Value> = songs.iter().map(song_to_child).collect();

    response::ok_json(json!({
        "searchResult3": {
            "artist": artist_jsons,
            "album": album_jsons,
            "song": song_jsons,
        }
    }))
}

pub async fn stream(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
    req: HttpRequest,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    let Some(id) = q.id.as_deref() else {
        return response::error_json(10, "Required parameter is missing: id");
    };
    let song = match repo::find_song(&state.pool, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return response::error_json(70, "Song not found"),
        Err(e) => {
            tracing::error!("stream lookup {id}: {e}");
            return response::error_json(0, "database error");
        }
    };
    let path = PathBuf::from(&song.path);
    let file_size = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::error!("stream stat {}: {e}", song.path);
            return response::error_json(0, "could not read file");
        }
    };

    if let Some(range_hdr) = req.headers().get(actix_web::http::header::RANGE) {
        if let Ok(range_str) = range_hdr.to_str() {
            if let Some(range) = range_str.strip_prefix("bytes=") {
                let mut parts = range.splitn(2, '-');
                let start: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let end: u64 = parts
                    .next()
                    .filter(|s| !s.is_empty())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(file_size.saturating_sub(1))
                    .min(file_size.saturating_sub(1));
                if start <= end && file_size > 0 {
                    use std::io::{Read, Seek, SeekFrom};
                    return match std::fs::File::open(&path) {
                        Ok(mut file) => {
                            let _ = file.seek(SeekFrom::Start(start));
                            let length = (end - start + 1) as usize;
                            let mut buf = vec![0u8; length];
                            let n = file.read(&mut buf).unwrap_or(0);
                            buf.truncate(n);
                            let actual_end = start + n as u64 - 1;
                            HttpResponse::PartialContent()
                                .content_type(song.content_type.clone())
                                .insert_header(("Accept-Ranges", "bytes"))
                                .insert_header(("Content-Length", n.to_string()))
                                .insert_header((
                                    "Content-Range",
                                    format!("bytes {}-{}/{}", start, actual_end, file_size),
                                ))
                                .body(buf)
                        }
                        Err(e) => {
                            tracing::error!("stream open {}: {e}", song.path);
                            response::error_json(0, "could not read file")
                        }
                    };
                }
            }
        }
    }

    match std::fs::read(&path) {
        Ok(data) => HttpResponse::Ok()
            .content_type(song.content_type)
            .insert_header(("Accept-Ranges", "bytes"))
            .insert_header(("Content-Length", file_size.to_string()))
            .body(data),
        Err(e) => {
            tracing::error!("stream read {}: {e}", song.path);
            response::error_json(0, "could not read file")
        }
    }
}

pub async fn get_cover_art(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    let Some(id) = q.id.as_deref() else {
        return HttpResponse::NotFound().finish();
    };

    let cover_filename = if id.starts_with("al-") {
        repo::find_album(&state.pool, id)
            .await
            .ok()
            .flatten()
            .and_then(|a| a.cover_art)
    } else if id.starts_with("so-") {
        let song = repo::find_song(&state.pool, id).await.ok().flatten();
        match song {
            Some(s) => {
                if s.cover_art.is_some() {
                    s.cover_art
                } else {
                    repo::find_album(&state.pool, &s.album_id)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|a| a.cover_art)
                }
            }
            None => None,
        }
    } else if id.starts_with("ar-") {
        let albums = repo::albums_by_artist(&state.pool, id)
            .await
            .unwrap_or_default();
        albums.into_iter().find_map(|a| a.cover_art)
    } else {
        None
    };

    let Some(filename) = cover_filename else {
        return HttpResponse::NotFound().finish();
    };
    let full = state.covers_dir.join(&filename);
    match std::fs::read(&full) {
        Ok(data) => {
            let mime = mime_guess::from_path(&full)
                .first_or_octet_stream()
                .to_string();
            HttpResponse::Ok().content_type(mime).body(data)
        }
        Err(e) => {
            tracing::warn!("getCoverArt read {}: {e}", full.display());
            HttpResponse::NotFound().finish()
        }
    }
}

pub async fn get_scan_status(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    let running = state.scan_progress.running.load(Ordering::SeqCst);
    let count = state.scan_progress.count.load(Ordering::SeqCst);
    response::ok_json(json!({
        "scanStatus": {
            "scanning": running,
            "count": count,
        }
    }))
}

pub async fn start_scan(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    if state.scan_progress.running.load(Ordering::SeqCst) {
        return response::ok_json(json!({
            "scanStatus": {
                "scanning": true,
                "count": state.scan_progress.count.load(Ordering::SeqCst),
            }
        }));
    }
    let pool = state.pool.clone();
    let music_dir = state.music_dir.clone();
    let covers_dir = state.covers_dir.clone();
    let progress = state.scan_progress.clone();
    tokio::spawn(async move {
        if let Err(e) = scanner::scan(pool, music_dir, covers_dir, progress).await {
            tracing::error!("scan: {e}");
        }
    });
    response::ok_json(json!({
        "scanStatus": { "scanning": true, "count": 0 }
    }))
}
