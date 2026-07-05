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

  Library — ID3 tag browsing
    GET  /rest/getArtists       alphabetical artist index
    GET  /rest/getArtist        ?id=ar-…   albums for an artist
    GET  /rest/getAlbum         ?id=al-…   songs for an album
    GET  /rest/getSong          ?id=so-…   single song lookup
    GET  /rest/getAlbumList2    ?type=alphabeticalByName|alphabeticalByArtist|newest|random
    GET  /rest/getAlbumList     alias of getAlbumList2

  Library — folder browsing
    GET  /rest/getIndexes        flat A-Z artist index
    GET  /rest/getMusicDirectory ?id=1|ar-…|al-…

  Genres
    GET  /rest/getGenres
    GET  /rest/getSongsByGenre  ?genre=…&count=&offset=

  Lists
    GET  /rest/getRandomSongs   ?size=&fromYear=&toYear=&genre=
    GET  /rest/getStarred2      starred artists / albums / songs
    GET  /rest/getStarred       alias of getStarred2

  Playback
    GET  /rest/stream           ?id=so-…   raw audio, Range supported
    GET  /rest/download         ?id=so-…   alias for /rest/stream
    GET  /rest/getCoverArt      ?id=al-…|ar-…|so-…   cached album art
    GET  /rest/scrobble         ?id=so-…
    GET  /rest/getNowPlaying
    GET  /rest/updateNowPlaying

  Search
    GET  /rest/search3          ?query=…&artistCount=&albumCount=&songCount=
    GET  /rest/search2          legacy alias

  Playlists
    GET  /rest/getPlaylists
    GET  /rest/getPlaylist      ?id=pl-…
    GET  /rest/createPlaylist   ?name=…&songId=…&songId=…   (or ?playlistId=… to replace)
    GET  /rest/updatePlaylist   ?playlistId=…&name=&comment=&songIdToAdd=&songIndexToRemove=
    GET  /rest/deletePlaylist   ?id=pl-…

  Starring
    GET  /rest/star             ?id=so-…|albumId=al-…|artistId=ar-…
    GET  /rest/unstar           ?id=so-…|albumId=al-…|artistId=ar-…

  Artist / album info  (minimal stubs — no Last.fm lookup)
    GET  /rest/getArtistInfo    ?id=ar-…
    GET  /rest/getArtistInfo2   ?id=ar-…
    GET  /rest/getAlbumInfo     ?id=al-…
    GET  /rest/getAlbumInfo2    ?id=al-…
    GET  /rest/getSimilarSongs  ?id=…
    GET  /rest/getSimilarSongs2 ?id=…
    GET  /rest/getTopSongs      ?artist=…&count=
    GET  /rest/getLyrics        ?artist=&title=

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

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct RandomSongsParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub size: Option<i64>,
    #[serde(rename = "fromYear")]
    pub from_year: Option<i64>,
    #[serde(rename = "toYear")]
    pub to_year: Option<i64>,
    pub genre: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct StarParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "albumId")]
    pub album_id: Option<String>,
    #[serde(rename = "artistId")]
    pub artist_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct SongsByGenreParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub genre: Option<String>,
    pub count: Option<i64>,
    pub offset: Option<i64>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct IndexesParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    #[serde(rename = "musicFolderId")]
    pub music_folder_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct TopSongsParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub artist: Option<String>,
    pub count: Option<i64>,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
pub struct LyricsParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub id: Option<String>,
    pub artist: Option<String>,
    pub title: Option<String>,
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
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(s.id));
    m.insert("parent".into(), json!(s.album_id));
    m.insert("isDir".into(), json!(false));
    m.insert("title".into(), json!(s.title));
    m.insert("album".into(), json!(s.album));
    m.insert("artist".into(), json!(s.artist));
    if let Some(t) = s.track_number {
        m.insert("track".into(), json!(t));
    }
    if let Some(y) = s.year {
        m.insert("year".into(), json!(y));
    }
    if let Some(g) = &s.genre {
        m.insert("genre".into(), json!(g));
    }
    m.insert("coverArt".into(), json!(s.album_id));
    m.insert("size".into(), json!(s.filesize));
    m.insert("contentType".into(), json!(s.content_type));
    m.insert("suffix".into(), json!(s.suffix));
    m.insert("duration".into(), json!(s.duration_ms / 1000));
    m.insert("bitRate".into(), json!(s.bitrate));
    m.insert("path".into(), json!(s.path));
    m.insert("isVideo".into(), json!(false));
    if let Some(d) = s.disc_number {
        m.insert("discNumber".into(), json!(d));
    }
    m.insert("albumId".into(), json!(s.album_id));
    m.insert("artistId".into(), json!(s.artist_id));
    m.insert("type".into(), json!("music"));
    Value::Object(m)
}

fn album_to_child(a: &Album, song_count: i64, duration_s: i64) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(a.id));
    m.insert("name".into(), json!(a.title));
    m.insert("title".into(), json!(a.title));
    m.insert("artist".into(), json!(a.artist));
    m.insert("artistId".into(), json!(a.artist_id));
    m.insert("songCount".into(), json!(song_count));
    m.insert("duration".into(), json!(duration_s));
    if a.year > 0 {
        m.insert("year".into(), json!(a.year));
    }
    m.insert("coverArt".into(), json!(a.id));
    m.insert("created".into(), json!("2020-01-01T00:00:00Z"));
    Value::Object(m)
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
    let albums = repo::albums_by_artist(&state.pool, id)
        .await
        .unwrap_or_default();
    let mut album_jsons = Vec::with_capacity(albums.len());
    for a in &albums {
        let count = repo::song_count_for_album(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id)
            .await
            .unwrap_or(0);
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
    let songs = repo::songs_by_album(&state.pool, id)
        .await
        .unwrap_or_default();
    let total_dur = songs.iter().map(|s| s.duration_ms / 1000).sum::<i64>();
    let song_jsons: Vec<Value> = songs.iter().map(song_to_child).collect();
    let mut a = serde_json::Map::new();
    a.insert("id".into(), json!(album.id));
    a.insert("name".into(), json!(album.title));
    a.insert("title".into(), json!(album.title));
    a.insert("artist".into(), json!(album.artist));
    a.insert("artistId".into(), json!(album.artist_id));
    a.insert("coverArt".into(), json!(album.id));
    a.insert("songCount".into(), json!(song_jsons.len()));
    a.insert("duration".into(), json!(total_dur));
    if album.year > 0 {
        a.insert("year".into(), json!(album.year));
    }
    a.insert("song".into(), json!(song_jsons));
    response::ok_json(json!({ "album": Value::Object(a) }))
}

pub async fn get_song(state: web::Data<SubsonicState>, query: web::Query<IdParam>) -> HttpResponse {
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
        let count = repo::song_count_for_album(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id)
            .await
            .unwrap_or(0);
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

    let ts = state.typesense.as_deref();
    let artists = repo::search_artists(&state.pool, &term, artist_limit, artist_offset, ts)
        .await
        .unwrap_or_default();
    let albums = repo::search_albums(&state.pool, &term, album_limit, album_offset, ts)
        .await
        .unwrap_or_default();
    let songs = repo::search_songs(&state.pool, &term, song_limit, song_offset, ts)
        .await
        .unwrap_or_default();

    let artist_jsons: Vec<Value> = artists.iter().map(|a| artist_to_json(a, 0)).collect();
    let mut album_jsons = Vec::with_capacity(albums.len());
    for a in &albums {
        let count = repo::song_count_for_album(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id)
            .await
            .unwrap_or(0);
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

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_repeated(qs: &str, key: &str) -> Vec<String> {
    qs.split('&')
        .filter_map(|p| {
            let mut split = p.splitn(2, '=');
            let k = split.next()?;
            if k != key {
                return None;
            }
            let v = split.next().unwrap_or("");
            Some(percent_decode(v))
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_value(qs: &str, key: &str) -> Option<String> {
    qs.split('&').find_map(|p| {
        let mut split = p.splitn(2, '=');
        let k = split.next()?;
        if k != key {
            return None;
        }
        Some(percent_decode(split.next().unwrap_or("")))
    })
}

fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn new_playlist_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut h = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut h);
    std::thread::current().id().hash(&mut h);
    format!("pl-{:016x}", h.finish())
}

fn auth_from_qs(state: &SubsonicState, qs: &str) -> Option<HttpResponse> {
    let u = query_value(qs, "u");
    let p = query_value(qs, "p");
    let t = query_value(qs, "t");
    let s = query_value(qs, "s");
    if !auth::check(
        &state.username,
        &state.password,
        u.as_deref(),
        p.as_deref(),
        t.as_deref(),
        s.as_deref(),
    ) {
        return Some(response::error_json(40, "Wrong username or password"));
    }
    None
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
    let ts = state.typesense.clone();
    tokio::spawn(async move {
        if let Err(e) = scanner::scan(pool, music_dir, covers_dir, progress, ts).await {
            tracing::error!("scan: {e}");
        }
    });
    response::ok_json(json!({
        "scanStatus": { "scanning": true, "count": 0 }
    }))
}

// ── Folder browsing ───────────────────────────────────────────────────────────

pub async fn get_indexes(
    state: web::Data<SubsonicState>,
    query: web::Query<IndexesParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(
        &state,
        &CommonLike {
            u: q.u.as_deref(),
            p: q.p.as_deref(),
            t: q.t.as_deref(),
            s: q.s.as_deref(),
        },
    ) {
        return r;
    }
    let artists = repo::all_artists(&state.pool).await.unwrap_or_default();
    let counts = repo::album_counts_by_artist(&state.pool)
        .await
        .unwrap_or_default();
    let index = build_artist_index(&artists, &counts);
    response::ok_json(json!({
        "indexes": {
            "lastModified": 0,
            "ignoredArticles": "The An A Die Das Ein",
            "index": index,
        }
    }))
}

pub async fn get_music_directory(
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

    if id == "1" {
        let artists = repo::all_artists(&state.pool).await.unwrap_or_default();
        let children: Vec<Value> = artists
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "parent": "1",
                    "isDir": true,
                    "title": a.name,
                    "album": a.name,
                    "artist": a.name,
                    "coverArt": a.id,
                })
            })
            .collect();
        return response::ok_json(json!({
            "directory": {
                "id": "1",
                "name": "Music",
                "child": children,
            }
        }));
    }

    if let Ok(Some(artist)) = repo::find_artist(&state.pool, id).await {
        let albums = repo::albums_by_artist(&state.pool, id)
            .await
            .unwrap_or_default();
        let children: Vec<Value> = albums
            .iter()
            .map(|a| {
                let mut m = serde_json::Map::new();
                m.insert("id".into(), json!(a.id));
                m.insert("parent".into(), json!(artist.id));
                m.insert("isDir".into(), json!(true));
                m.insert("title".into(), json!(a.title));
                m.insert("album".into(), json!(a.title));
                m.insert("artist".into(), json!(a.artist));
                if a.year > 0 {
                    m.insert("year".into(), json!(a.year));
                }
                m.insert("coverArt".into(), json!(a.id));
                Value::Object(m)
            })
            .collect();
        return response::ok_json(json!({
            "directory": {
                "id": artist.id,
                "name": artist.name,
                "child": children,
            }
        }));
    }

    if let Ok(Some(album)) = repo::find_album(&state.pool, id).await {
        let songs = repo::songs_by_album(&state.pool, id)
            .await
            .unwrap_or_default();
        let children: Vec<Value> = songs.iter().map(song_to_child).collect();
        return response::ok_json(json!({
            "directory": {
                "id": album.id,
                "name": album.title,
                "child": children,
            }
        }));
    }

    response::error_json(70, "Directory not found")
}

// ── Genres ────────────────────────────────────────────────────────────────────

pub async fn get_genres(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    let rows = repo::distinct_genres(&state.pool).await.unwrap_or_default();
    let genres: Vec<Value> = rows
        .into_iter()
        .map(|(name, songs, albums)| {
            json!({ "value": name, "songCount": songs, "albumCount": albums })
        })
        .collect();
    response::ok_json(json!({ "genres": { "genre": genres } }))
}

pub async fn get_songs_by_genre(
    state: web::Data<SubsonicState>,
    query: web::Query<SongsByGenreParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(
        &state,
        &CommonLike {
            u: q.u.as_deref(),
            p: q.p.as_deref(),
            t: q.t.as_deref(),
            s: q.s.as_deref(),
        },
    ) {
        return r;
    }
    let Some(genre) = q.genre.as_deref() else {
        return response::error_json(10, "Required parameter is missing: genre");
    };
    let count = q.count.unwrap_or(10).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let songs = repo::songs_by_genre(&state.pool, genre, count, offset)
        .await
        .unwrap_or_default();
    let children: Vec<Value> = songs.iter().map(song_to_child).collect();
    response::ok_json(json!({ "songsByGenre": { "song": children } }))
}

// ── Random / Starred ──────────────────────────────────────────────────────────

pub async fn get_random_songs(
    state: web::Data<SubsonicState>,
    query: web::Query<RandomSongsParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(
        &state,
        &CommonLike {
            u: q.u.as_deref(),
            p: q.p.as_deref(),
            t: q.t.as_deref(),
            s: q.s.as_deref(),
        },
    ) {
        return r;
    }
    let size = q.size.unwrap_or(10).clamp(1, 500);
    let songs = repo::random_songs(
        &state.pool,
        size,
        q.from_year,
        q.to_year,
        q.genre.as_deref(),
    )
    .await
    .unwrap_or_default();
    let children: Vec<Value> = songs.iter().map(song_to_child).collect();
    response::ok_json(json!({ "randomSongs": { "song": children } }))
}

pub async fn get_starred2(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    let songs = repo::starred_songs(&state.pool).await.unwrap_or_default();
    let albums = repo::starred_albums(&state.pool).await.unwrap_or_default();
    let artists = repo::starred_artists(&state.pool).await.unwrap_or_default();

    let song_jsons: Vec<Value> = songs
        .iter()
        .map(|(s, when)| {
            let mut v = song_to_child(s);
            if let Some(obj) = v.as_object_mut() {
                obj.insert("starred".into(), json!(when));
            }
            v
        })
        .collect();
    let mut album_jsons = Vec::with_capacity(albums.len());
    for (a, when) in &albums {
        let count = repo::song_count_for_album(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        let mut v = album_to_child(a, count, dur);
        if let Some(obj) = v.as_object_mut() {
            obj.insert("starred".into(), json!(when));
        }
        album_jsons.push(v);
    }
    let artist_jsons: Vec<Value> = artists
        .iter()
        .map(|(a, when)| {
            let mut v = artist_to_json(a, 0);
            if let Some(obj) = v.as_object_mut() {
                obj.insert("starred".into(), json!(when));
            }
            v
        })
        .collect();

    response::ok_json(json!({
        "starred2": {
            "artist": artist_jsons,
            "album": album_jsons,
            "song": song_jsons,
        }
    }))
}

pub async fn get_starred(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    // Subsonic spec wraps under "starred" — same payload shape.
    let res = get_starred2(state, query).await;
    res
}

pub async fn star(state: web::Data<SubsonicState>, query: web::Query<StarParams>) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(
        &state,
        &CommonLike {
            u: q.u.as_deref(),
            p: q.p.as_deref(),
            t: q.t.as_deref(),
            s: q.s.as_deref(),
        },
    ) {
        return r;
    }
    let target =
        q.id.as_deref()
            .or(q.album_id.as_deref())
            .or(q.artist_id.as_deref());
    let Some(target) = target else {
        return response::ok_json(json!({}));
    };
    if let Err(e) = repo::star(&state.pool, target, &now_iso8601()).await {
        tracing::error!("star: {e}");
        return response::error_json(0, "database error");
    }
    response::ok_json(json!({}))
}

pub async fn unstar(
    state: web::Data<SubsonicState>,
    query: web::Query<StarParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(
        &state,
        &CommonLike {
            u: q.u.as_deref(),
            p: q.p.as_deref(),
            t: q.t.as_deref(),
            s: q.s.as_deref(),
        },
    ) {
        return r;
    }
    let target =
        q.id.as_deref()
            .or(q.album_id.as_deref())
            .or(q.artist_id.as_deref());
    let Some(target) = target else {
        return response::ok_json(json!({}));
    };
    if let Err(e) = repo::unstar(&state.pool, target).await {
        tracing::error!("unstar: {e}");
        return response::error_json(0, "database error");
    }
    response::ok_json(json!({}))
}

// ── Scrobble / NowPlaying ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ScrobbleParams {
    pub u: Option<String>,
    pub p: Option<String>,
    pub t: Option<String>,
    pub s: Option<String>,
    pub f: Option<String>,
    pub id: Option<String>,
    /// Milliseconds since epoch; when omitted we compute `now - duration`.
    pub time: Option<i64>,
    /// `false` → playing-now update; anything else (default) → full listen.
    /// Subsonic spec: default is `true`.
    pub submission: Option<bool>,
}

pub async fn scrobble(
    state: web::Data<SubsonicState>,
    query: web::Query<ScrobbleParams>,
) -> HttpResponse {
    let q = query.into_inner();
    let common = IdParam {
        u: q.u.clone(),
        p: q.p.clone(),
        t: q.t.clone(),
        s: q.s.clone(),
        f: q.f.clone(),
        id: q.id.clone(),
    };
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&common)) {
        return r;
    }

    // Forward to ListenBrainz when a plugin is configured. Failure is
    // silent to the client — scrobbling is best-effort.
    if let (Some(client), Some(id)) = (state.scrobble.as_ref(), q.id.as_deref()) {
        if let Ok(Some(song)) = repo::find_song(&state.pool, id).await {
            let album = if song.album.is_empty() {
                None
            } else {
                Some(song.album.as_str())
            };
            let meta = crate::scrobble::TrackMeta {
                artist: &song.artist,
                track: &song.title,
                album,
            };
            let is_submission = q.submission.unwrap_or(true);
            if is_submission {
                // Subsonic `time` is ms since epoch (playback start). Fall
                // back to now-minus-duration when the client omits it.
                let listened_at = q
                    .time
                    .map(|ms| ms / 1000)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp() - song.duration_ms / 1000);
                client.submit_listen(meta, listened_at).await;
            } else {
                client.submit_playing_now(meta).await;
            }
        }
    }

    response::ok_json(json!({}))
}

pub async fn get_now_playing(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    response::ok_json(json!({ "nowPlaying": { "entry": [] } }))
}

pub async fn update_now_playing(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    response::ok_json(json!({}))
}

// ── Playlists ─────────────────────────────────────────────────────────────────

async fn playlist_json(state: &SubsonicState, pl: &crate::models::Playlist) -> Value {
    let songs = repo::playlist_songs(&state.pool, &pl.id)
        .await
        .unwrap_or_default();
    let duration = songs.iter().map(|s| s.duration_ms / 1000).sum::<i64>();
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(pl.id));
    m.insert("name".into(), json!(pl.name));
    if let Some(c) = &pl.comment {
        m.insert("comment".into(), json!(c));
    }
    m.insert("songCount".into(), json!(songs.len()));
    m.insert("duration".into(), json!(duration));
    m.insert("public".into(), json!(pl.public != 0));
    m.insert("created".into(), json!(pl.created_at));
    m.insert("changed".into(), json!(pl.updated_at));
    Value::Object(m)
}

pub async fn get_playlists(
    state: web::Data<SubsonicState>,
    query: web::Query<CommonParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_common(&q)) {
        return r;
    }
    let playlists = repo::all_playlists(&state.pool).await.unwrap_or_default();
    let mut out = Vec::with_capacity(playlists.len());
    for pl in &playlists {
        out.push(playlist_json(&state, pl).await);
    }
    response::ok_json(json!({ "playlists": { "playlist": out } }))
}

pub async fn get_playlist(
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
    let pl = match repo::find_playlist(&state.pool, id).await {
        Ok(Some(p)) => p,
        Ok(None) => return response::error_json(70, "Playlist not found"),
        Err(e) => {
            tracing::error!("getPlaylist: {e}");
            return response::error_json(0, "database error");
        }
    };
    let songs = repo::playlist_songs(&state.pool, id)
        .await
        .unwrap_or_default();
    let duration = songs.iter().map(|s| s.duration_ms / 1000).sum::<i64>();
    let entry: Vec<Value> = songs.iter().map(song_to_child).collect();
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(pl.id));
    m.insert("name".into(), json!(pl.name));
    if let Some(c) = &pl.comment {
        m.insert("comment".into(), json!(c));
    }
    m.insert("songCount".into(), json!(entry.len()));
    m.insert("duration".into(), json!(duration));
    m.insert("public".into(), json!(pl.public != 0));
    m.insert("created".into(), json!(pl.created_at));
    m.insert("changed".into(), json!(pl.updated_at));
    m.insert("entry".into(), json!(entry));
    response::ok_json(json!({ "playlist": Value::Object(m) }))
}

pub async fn create_playlist(state: web::Data<SubsonicState>, req: HttpRequest) -> HttpResponse {
    let qs = req.query_string();
    if let Some(r) = auth_from_qs(&state, qs) {
        return r;
    }
    let existing_id = query_value(qs, "playlistId");
    let name = query_value(qs, "name").unwrap_or_else(|| "Untitled".to_string());
    let song_ids = parse_repeated(qs, "songId");
    let now = now_iso8601();

    let id = match existing_id {
        Some(id) if !id.is_empty() => {
            if repo::find_playlist(&state.pool, &id)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                if let Err(e) = repo::create_playlist(&state.pool, &id, &name, &now).await {
                    tracing::error!("createPlaylist: {e}");
                    return response::error_json(0, "database error");
                }
            } else {
                let _ = repo::rename_playlist(&state.pool, &id, &name, &now).await;
            }
            if !song_ids.is_empty() {
                let _ = repo::replace_playlist_songs(&state.pool, &id, &song_ids).await;
            }
            id
        }
        _ => {
            let id = new_playlist_id();
            if let Err(e) = repo::create_playlist(&state.pool, &id, &name, &now).await {
                tracing::error!("createPlaylist: {e}");
                return response::error_json(0, "database error");
            }
            if !song_ids.is_empty() {
                let _ = repo::append_playlist_songs(&state.pool, &id, &song_ids).await;
            }
            id
        }
    };

    let pl = match repo::find_playlist(&state.pool, &id).await {
        Ok(Some(p)) => p,
        _ => return response::error_json(0, "playlist persistence error"),
    };
    let songs = repo::playlist_songs(&state.pool, &id)
        .await
        .unwrap_or_default();
    let duration = songs.iter().map(|s| s.duration_ms / 1000).sum::<i64>();
    let entry: Vec<Value> = songs.iter().map(song_to_child).collect();
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(pl.id));
    m.insert("name".into(), json!(pl.name));
    if let Some(c) = &pl.comment {
        m.insert("comment".into(), json!(c));
    }
    m.insert("songCount".into(), json!(entry.len()));
    m.insert("duration".into(), json!(duration));
    m.insert("public".into(), json!(pl.public != 0));
    m.insert("created".into(), json!(pl.created_at));
    m.insert("changed".into(), json!(pl.updated_at));
    m.insert("entry".into(), json!(entry));
    response::ok_json(json!({ "playlist": Value::Object(m) }))
}

pub async fn update_playlist(state: web::Data<SubsonicState>, req: HttpRequest) -> HttpResponse {
    let qs = req.query_string();
    if let Some(r) = auth_from_qs(&state, qs) {
        return r;
    }
    let Some(id) = query_value(qs, "playlistId") else {
        return response::error_json(10, "Required parameter is missing: playlistId");
    };
    if repo::find_playlist(&state.pool, &id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return response::error_json(70, "Playlist not found");
    }
    let now = now_iso8601();
    if let Some(name) = query_value(qs, "name") {
        let _ = repo::rename_playlist(&state.pool, &id, &name, &now).await;
    }
    if let Some(comment) = query_value(qs, "comment") {
        let _ = repo::set_playlist_comment(&state.pool, &id, &comment, &now).await;
    }
    let to_add = parse_repeated(qs, "songIdToAdd");
    if !to_add.is_empty() {
        let _ = repo::append_playlist_songs(&state.pool, &id, &to_add).await;
    }
    let to_remove: Vec<usize> = parse_repeated(qs, "songIndexToRemove")
        .into_iter()
        .filter_map(|s| s.parse::<usize>().ok())
        .collect();
    if !to_remove.is_empty() {
        let mut current = repo::playlist_song_ids(&state.pool, &id)
            .await
            .unwrap_or_default();
        let mut indices: Vec<usize> = to_remove;
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for i in indices {
            if i < current.len() {
                current.remove(i);
            }
        }
        let _ = repo::replace_playlist_songs(&state.pool, &id, &current).await;
    }
    response::ok_json(json!({}))
}

pub async fn delete_playlist(
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
    if let Err(e) = repo::delete_playlist(&state.pool, id).await {
        tracing::error!("deletePlaylist: {e}");
        return response::error_json(0, "database error");
    }
    response::ok_json(json!({}))
}

// ── Artist / Album info — minimal stubs ───────────────────────────────────────

fn empty_artist_info() -> Value {
    json!({
        "biography": "",
        "musicBrainzId": "",
        "lastFmUrl": "",
        "smallImageUrl": "",
        "mediumImageUrl": "",
        "largeImageUrl": "",
        "similarArtist": [],
    })
}

pub async fn get_artist_info(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    response::ok_json(json!({ "artistInfo": empty_artist_info() }))
}

pub async fn get_artist_info2(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    response::ok_json(json!({ "artistInfo2": empty_artist_info() }))
}

pub async fn get_album_info(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    response::ok_json(json!({
        "albumInfo": {
            "notes": "",
            "musicBrainzId": "",
            "lastFmUrl": "",
            "smallImageUrl": "",
            "mediumImageUrl": "",
            "largeImageUrl": "",
        }
    }))
}

pub async fn get_similar_songs(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    response::ok_json(json!({ "similarSongs": { "song": [] } }))
}

pub async fn get_similar_songs2(
    state: web::Data<SubsonicState>,
    query: web::Query<IdParam>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(&state, &CommonLike::from_id(&q)) {
        return r;
    }
    response::ok_json(json!({ "similarSongs2": { "song": [] } }))
}

pub async fn get_top_songs(
    state: web::Data<SubsonicState>,
    query: web::Query<TopSongsParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(
        &state,
        &CommonLike {
            u: q.u.as_deref(),
            p: q.p.as_deref(),
            t: q.t.as_deref(),
            s: q.s.as_deref(),
        },
    ) {
        return r;
    }
    let Some(artist) = q.artist.as_deref() else {
        return response::ok_json(json!({ "topSongs": { "song": [] } }));
    };
    let count = q.count.unwrap_or(50).clamp(1, 500);
    let songs = repo::search_songs(&state.pool, artist, count, 0, state.typesense.as_deref())
        .await
        .unwrap_or_default();
    let children: Vec<Value> = songs
        .iter()
        .filter(|s| s.artist.eq_ignore_ascii_case(artist))
        .map(song_to_child)
        .collect();
    response::ok_json(json!({ "topSongs": { "song": children } }))
}

pub async fn get_lyrics(
    state: web::Data<SubsonicState>,
    query: web::Query<LyricsParams>,
) -> HttpResponse {
    let q = query.into_inner();
    if let Some(r) = require_auth(
        &state,
        &CommonLike {
            u: q.u.as_deref(),
            p: q.p.as_deref(),
            t: q.t.as_deref(),
            s: q.s.as_deref(),
        },
    ) {
        return r;
    }
    response::ok_json(json!({
        "lyrics": {
            "artist": q.artist.unwrap_or_default(),
            "title": q.title.unwrap_or_default(),
            "value": "",
        }
    }))
}

// ── Aliases ───────────────────────────────────────────────────────────────────

pub async fn get_album_list(
    state: web::Data<SubsonicState>,
    query: web::Query<AlbumListParams>,
) -> HttpResponse {
    // Same shape as albumList2 — most clients accept this.
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
        let count = repo::song_count_for_album(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        album_jsons.push(album_to_child(a, count, dur));
    }
    response::ok_json(json!({ "albumList": { "album": album_jsons } }))
}

pub async fn search2(
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

    let ts = state.typesense.as_deref();
    let artists = repo::search_artists(&state.pool, &term, artist_limit, artist_offset, ts)
        .await
        .unwrap_or_default();
    let albums = repo::search_albums(&state.pool, &term, album_limit, album_offset, ts)
        .await
        .unwrap_or_default();
    let songs = repo::search_songs(&state.pool, &term, song_limit, song_offset, ts)
        .await
        .unwrap_or_default();
    let artist_jsons: Vec<Value> = artists.iter().map(|a| artist_to_json(a, 0)).collect();
    let mut album_jsons = Vec::with_capacity(albums.len());
    for a in &albums {
        let count = repo::song_count_for_album(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        let dur = repo::songs_for_album_duration(&state.pool, &a.id)
            .await
            .unwrap_or(0);
        album_jsons.push(album_to_child(a, count, dur));
    }
    let song_jsons: Vec<Value> = songs.iter().map(song_to_child).collect();

    response::ok_json(json!({
        "searchResult2": {
            "artist": artist_jsons,
            "album": album_jsons,
            "song": song_jsons,
        }
    }))
}
