use crate::models::{Album, Artist, Playlist, Song, Video};
use crate::server::repo;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::auth::{self, AuthedUser, EmbyAuth};
use super::dto::{
    AuthenticationResult, BaseItemDto, ImageBlurHashes, ImageTags, ItemsResult, MediaSource,
    MediaStream, NameGuidPair, PlaybackInfoResponse, PlaylistCreationResult, PublicSystemInfo,
    SessionInfoDto, SystemInfo, UserConfiguration, UserDto, UserItemDataDto, UserPolicy,
    ViewsResult, JELLYFIN_API_VERSION,
};
use super::mapping;
use super::JellyfinState;

const TICKS_PER_MS: i64 = 10_000;

fn now_iso() -> String {
    // Emit the *naive* 7-digit format real Jellyfin uses
    // (`2026-06-29T12:17:19.6620000`) — no timezone marker.
    // jellyfin-sdk-kotlin deserializes via LocalDateTime, which rejects
    // a trailing `Z` or `+00:00` and silently drops the whole object.
    let now = Utc::now();
    let ticks = now.timestamp_subsec_nanos() / 100; // 100-ns "ticks", 7 digits
    format!("{}.{ticks:07}", now.format("%Y-%m-%dT%H:%M:%S"))
}

fn parse_auth(req: &HttpRequest) -> EmbyAuth {
    let headers = req.headers();
    for name in ["x-emby-authorization", "authorization"] {
        if let Some(v) = headers.get(name) {
            if let Ok(s) = v.to_str() {
                return auth::parse_emby_auth_header(s);
            }
        }
    }
    EmbyAuth::default()
}

fn server_base(state: &JellyfinState, req: &HttpRequest) -> String {
    if let Some(h) = req.headers().get("host").and_then(|v| v.to_str().ok()) {
        let scheme = req.connection_info().scheme().to_string();
        return format!("{scheme}://{h}");
    }
    format!("http://{}:{}", state.host, state.port)
}

// ── Root index ────────────────────────────────────────────────────────────────

pub async fn index(state: web::Data<JellyfinState>) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body(format!(
            "smolsonic — Jellyfin-compatible sidecar\nserver: {} ({})\n",
            state.server_name, state.server_id
        ))
}

// ── System ───────────────────────────────────────────────────────────────────

/// Map Rust's `std::env::consts::OS` to the capitalised names Jellyfin uses
/// in `OperatingSystem` (e.g. `"Darwin"`, `"Linux"`, `"Windows"`). Some
/// clients pattern-match on this.
fn jellyfin_os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Darwin",
        "linux" => "Linux",
        "windows" => "Windows",
        "freebsd" => "FreeBSD",
        "openbsd" => "OpenBSD",
        "netbsd" => "NetBSD",
        "android" => "Android",
        "ios" => "iOS",
        other => match other.chars().next() {
            Some(c) if c.is_ascii_lowercase() => {
                Box::leak(format!("{}{}", c.to_ascii_uppercase(), &other[1..]).into_boxed_str())
            }
            _ => other,
        },
    }
}

fn public_info(state: &JellyfinState, req: &HttpRequest) -> PublicSystemInfo {
    PublicSystemInfo {
        local_address: Some(server_base(state, req)),
        server_name: Some(state.server_name.clone()),
        version: Some(JELLYFIN_API_VERSION.to_string()),
        product_name: Some("Jellyfin Server".to_string()),
        operating_system: Some(jellyfin_os_name().to_string()),
        id: Some(state.server_id.clone()),
        startup_wizard_completed: Some(true),
    }
}

pub async fn system_info_public(state: web::Data<JellyfinState>, req: HttpRequest) -> HttpResponse {
    HttpResponse::Ok().json(public_info(&state, &req))
}

pub async fn system_info(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let pub_info = public_info(&state, &req);
    let info = SystemInfo {
        local_address: pub_info.local_address,
        server_name: pub_info.server_name,
        version: pub_info.version,
        product_name: pub_info.product_name,
        operating_system: pub_info.operating_system,
        id: pub_info.id,
        startup_wizard_completed: pub_info.startup_wizard_completed,
        operating_system_display_name: Some(jellyfin_os_name().to_string()),
        package_name: Some("smolsonic".to_string()),
        has_pending_restart: false,
        is_shutting_down: false,
        supports_library_monitor: true,
        web_socket_port_number: state.port as i32,
        completed_installations: Some(vec![]),
        can_self_restart: false,
        can_launch_web_browser: false,
        program_data_path: Some(String::new()),
        web_path: Some(String::new()),
        items_by_name_path: Some(String::new()),
        cache_path: Some(String::new()),
        log_path: Some(String::new()),
        internal_metadata_path: Some(String::new()),
        transcoding_temp_path: Some(String::new()),
        cast_receiver_applications: Some(vec![]),
        has_update_available: false,
        encoder_location: Some("Default".to_string()),
        system_architecture: Some(std::env::consts::ARCH.to_string()),
    };
    HttpResponse::Ok().json(info)
}

pub async fn system_endpoint(_user: AuthedUser, req: HttpRequest) -> HttpResponse {
    let info = req.connection_info();
    let addr = info.realip_remote_addr().unwrap_or("");
    HttpResponse::Ok().json(json!({
        "IsLocal": true,
        "IsInNetwork": true,
        "RemoteAddress": addr,
    }))
}

// ── Users / Auth ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateBody {
    pub username: Option<String>,
    pub pw: Option<String>,
    pub password: Option<String>,
}

fn build_user(state: &JellyfinState) -> UserDto {
    UserDto {
        name: Some(state.username.as_str().to_string()),
        server_id: Some(state.server_id.clone()),
        server_name: Some(state.server_name.clone()),
        id: state.user_id.as_str().to_string(),
        primary_image_tag: None,
        primary_image_aspect_ratio: None,
        has_password: Some(true),
        has_configured_password: Some(true),
        has_configured_easy_password: Some(false),
        enable_auto_login: Some(false),
        last_login_date: Some(now_iso()),
        last_activity_date: Some(now_iso()),
        configuration: Some(UserConfiguration::default()),
        policy: Some(UserPolicy::admin()),
    }
}

pub async fn authenticate_by_name(
    state: web::Data<JellyfinState>,
    req: HttpRequest,
    body: web::Json<AuthenticateBody>,
) -> HttpResponse {
    let body = body.into_inner();
    let username = body.username.unwrap_or_default();
    let password = body.pw.or(body.password).unwrap_or_default();

    if username != *state.username || password != *state.password {
        return HttpResponse::Unauthorized().json(json!({
            "Message": "Invalid username or password"
        }));
    }

    let parsed = parse_auth(&req);
    let token = auth::random_hex(16);
    let now = now_iso();
    if let Err(e) =
        auth::store_token(&state.pool, &token, state.user_id.as_str(), &parsed, &now).await
    {
        tracing::error!("jellyfin: store_token: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    let user = build_user(&state);
    let session = SessionInfoDto {
        play_state: None,
        additional_users: Some(vec![]),
        capabilities: None,
        remote_end_point: Some(
            req.connection_info()
                .realip_remote_addr()
                .unwrap_or("")
                .to_string(),
        ),
        playable_media_types: vec!["Audio".into(), "Video".into()],
        id: Some(auth::random_hex(16)),
        user_id: user.id.clone(),
        user_name: user.name.clone(),
        client: parsed.client.clone(),
        last_activity_date: now.clone(),
        last_playback_check_in: now.clone(),
        last_paused_date: None,
        device_name: parsed.device.clone(),
        device_type: None,
        now_playing_item: None,
        now_viewing_item: None,
        device_id: parsed.device_id.clone(),
        application_version: parsed.version.clone(),
        transcoding_info: None,
        is_active: true,
        supports_media_control: false,
        supports_remote_control: false,
        now_playing_queue: Some(vec![]),
        has_custom_device_name: false,
        playlist_item_id: None,
        server_id: Some(state.server_id.clone()),
        user_primary_image_tag: None,
        supported_commands: vec![],
    };

    HttpResponse::Ok().json(AuthenticationResult {
        user: Some(user),
        session_info: Some(session),
        access_token: Some(token),
        server_id: Some(state.server_id.clone()),
    })
}

pub async fn users_public(state: web::Data<JellyfinState>) -> HttpResponse {
    // Most clients (Findroid, Streamyfin, official) show a user picker from
    // this list and then prompt for the password. Returning an empty array
    // makes them display "no users found" with no manual-login fallback.
    HttpResponse::Ok().json(vec![build_user(&state)])
}

pub async fn users_list(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    HttpResponse::Ok().json(vec![build_user(&state)])
}

pub async fn users_me(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    HttpResponse::Ok().json(build_user(&state))
}

pub async fn user_by_id(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    _path: web::Path<String>,
) -> HttpResponse {
    HttpResponse::Ok().json(build_user(&state))
}

// ── Views (top-level library list) ───────────────────────────────────────────

fn music_library_view(state: &JellyfinState) -> BaseItemDto {
    BaseItemDto {
        id: mapping::library_guid(),
        server_id: Some(state.server_id.clone()),
        name: Some("Music".to_string()),
        item_type: "CollectionFolder",
        media_type: "Unknown",
        is_folder: Some(true),
        collection_type: Some("music"),
        location_type: Some("FileSystem"),
        ..Default::default()
    }
}

fn movies_library_view(state: &JellyfinState) -> Option<BaseItemDto> {
    let name = state.video_library_name.as_ref()?.clone();
    Some(BaseItemDto {
        id: mapping::movies_library_guid(),
        server_id: Some(state.server_id.clone()),
        name: Some(name),
        item_type: "CollectionFolder",
        media_type: "Unknown",
        is_folder: Some(true),
        collection_type: Some("movies"),
        location_type: Some("FileSystem"),
        ..Default::default()
    })
}

/// Virtual "Playlists" library. Jellyfin's reference server auto-creates
/// this view so clients (Moonfin, Findroid, official web) render playlists
/// as a top-level tile alongside Music and Movies. `CollectionType` is the
/// spec enum value `"playlists"` (plural).
fn playlists_library_view(state: &JellyfinState) -> BaseItemDto {
    BaseItemDto {
        id: mapping::playlists_library_guid(),
        server_id: Some(state.server_id.clone()),
        name: Some("Playlists".to_string()),
        item_type: "CollectionFolder",
        media_type: "Unknown",
        is_folder: Some(true),
        collection_type: Some("playlists"),
        location_type: Some("FileSystem"),
        ..Default::default()
    }
}

fn all_library_views(state: &JellyfinState) -> Vec<BaseItemDto> {
    let mut v = vec![music_library_view(state)];
    if let Some(view) = movies_library_view(state) {
        v.push(view);
    }
    v.push(playlists_library_view(state));
    v
}

pub async fn user_views(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    _path: web::Path<String>,
) -> HttpResponse {
    let views = all_library_views(&state);
    let total = views.len() as i32;
    HttpResponse::Ok().json(ViewsResult {
        items: views,
        total_record_count: total,
        start_index: 0,
    })
}

pub async fn media_folders(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    let views = all_library_views(&state);
    let total = views.len() as i32;
    HttpResponse::Ok().json(ViewsResult {
        items: views,
        total_record_count: total,
        start_index: 0,
    })
}

pub async fn library_virtual_folders(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
) -> HttpResponse {
    let mut folders = vec![json!({
        "Name": "Music",
        "Locations": [state.music_dir.to_string_lossy()],
        "CollectionType": "music",
        "ItemId": mapping::library_guid(),
    })];
    if let Some(name) = state.video_library_name.as_ref() {
        folders.push(json!({
            "Name": name,
            "Locations": [],
            "CollectionType": "movies",
            "ItemId": mapping::movies_library_guid(),
        }));
    }
    folders.push(json!({
        "Name": "Playlists",
        "Locations": [],
        "CollectionType": "playlists",
        "ItemId": mapping::playlists_library_guid(),
    }));
    HttpResponse::Ok().json(folders)
}

// ── Items ────────────────────────────────────────────────────────────────────

/// Jellyfin query parameters use **camelCase** (`parentId`, `userId`,
/// `searchTerm`, …) — JSON bodies use PascalCase, queries don't.
///
/// We parse this by hand from `HttpRequest::query_string()` instead of using
/// `web::Query<ItemsQuery>` because the Jellyfin spec allows array params to
/// be sent as repeated keys (`?includeItemTypes=Folder&includeItemTypes=Movie`)
/// — actix's default `serde_urlencoded` rejects duplicates with 400. We
/// concatenate repeated values with commas, which our downstream `includes`
/// helper already understands.
#[derive(Debug, Default)]
#[allow(dead_code)] // Fields kept for spec completeness even if not consulted yet.
pub struct ItemsQuery {
    pub parent_id: Option<String>,
    pub include_item_types: Option<String>,
    pub media_types: Option<String>,
    pub name_starts_with: Option<String>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_less_than: Option<String>,
    pub recursive: Option<bool>,
    pub search_term: Option<String>,
    pub ids: Option<String>,
    pub album_artist_ids: Option<String>,
    pub artist_ids: Option<String>,
    pub album_ids: Option<String>,
    pub start_index: Option<i64>,
    pub limit: Option<i64>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub user_id: Option<String>,
    pub enable_user_data: Option<bool>,
    pub enable_total_record_count: Option<bool>,
    pub enable_images: Option<bool>,
    pub fields: Option<String>,
    pub is_favorite: Option<bool>,
    pub filters: Option<String>,
}

fn collect_query(req: &HttpRequest) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for pair in req.query_string().split('&').filter(|s| !s.is_empty()) {
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or("");
        let val = it.next().unwrap_or("");
        let decoded = urlencoding::decode(val)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| val.to_string());
        out.entry(key.to_string()).or_default().push(decoded);
    }
    out
}

fn parse_items_query(req: &HttpRequest) -> ItemsQuery {
    let q = collect_query(req);
    let one = |k: &str| q.get(k).and_then(|v| v.first()).cloned();
    let csv = |k: &str| q.get(k).map(|v| v.join(","));
    let parse_bool = |k: &str| one(k).and_then(|s| s.parse::<bool>().ok());
    let parse_i64 = |k: &str| one(k).and_then(|s| s.parse::<i64>().ok());
    ItemsQuery {
        parent_id: one("parentId").or_else(|| one("ParentId")),
        include_item_types: csv("includeItemTypes").or_else(|| csv("IncludeItemTypes")),
        media_types: csv("mediaTypes").or_else(|| csv("MediaTypes")),
        name_starts_with: one("nameStartsWith").or_else(|| one("NameStartsWith")),
        name_starts_with_or_greater: one("nameStartsWithOrGreater")
            .or_else(|| one("NameStartsWithOrGreater")),
        name_less_than: one("nameLessThan").or_else(|| one("NameLessThan")),
        recursive: parse_bool("recursive"),
        search_term: one("searchTerm").or_else(|| one("SearchTerm")),
        ids: csv("ids").or_else(|| csv("Ids")),
        album_artist_ids: csv("albumArtistIds").or_else(|| csv("AlbumArtistIds")),
        artist_ids: csv("artistIds").or_else(|| csv("ArtistIds")),
        album_ids: csv("albumIds").or_else(|| csv("AlbumIds")),
        start_index: parse_i64("startIndex").or_else(|| parse_i64("StartIndex")),
        limit: parse_i64("limit").or_else(|| parse_i64("Limit")),
        sort_by: one("sortBy").or_else(|| one("SortBy")),
        sort_order: one("sortOrder").or_else(|| one("SortOrder")),
        user_id: one("userId").or_else(|| one("UserId")),
        enable_user_data: parse_bool("enableUserData"),
        enable_total_record_count: parse_bool("enableTotalRecordCount"),
        enable_images: parse_bool("enableImages"),
        fields: csv("fields").or_else(|| csv("Fields")),
        is_favorite: parse_bool("isFavorite").or_else(|| parse_bool("IsFavorite")),
        filters: csv("filters").or_else(|| csv("Filters")),
    }
}

/// True when the client asked for favorites-only via either
/// `?IsFavorite=true` (per the OpenAPI `isFavorite` param) or
/// `?Filters=IsFavorite` (per the OpenAPI `filters` CSV enum, which also
/// includes `IsPlayed`, `IsResumable`, etc.). We only honour the affirmative
/// direction — `IsFavorite=false` is a "don't filter" no-op, matching how
/// most clients construct the URL.
fn wants_favorites_only(q: &ItemsQuery) -> bool {
    if q.is_favorite == Some(true) {
        return true;
    }
    includes(&q.filters, "IsFavorite")
}

/// Build a `UserItemDataDto` by folding together the `starred` sidecar and
/// the `user_item_data` row for `native_id`. `jf_guid` is the Jellyfin GUID
/// we emit in `Key`/`ItemId` (clients use it to correlate the DTO with the
/// containing BaseItem). Every `*_to_dto` helper — and the standalone
/// `/UserItems/{itemId}/UserData` GET — uses this so the fields stay in
/// sync with disk state.
async fn build_user_data(
    state: &JellyfinState,
    native_id: &str,
    jf_guid: String,
) -> UserItemDataDto {
    let is_favorite = repo::is_starred(&state.pool, native_id)
        .await
        .unwrap_or(false);
    let data = repo::get_user_item_data(&state.pool, native_id)
        .await
        .unwrap_or_default();
    UserItemDataDto {
        rating: data.rating,
        played_percentage: None,
        unplayed_item_count: None,
        playback_position_ticks: data.playback_position_ticks,
        play_count: data.play_count,
        is_favorite,
        likes: data.likes,
        last_played_date: data.last_played_date,
        played: data.played,
        key: jf_guid.clone(),
        item_id: jf_guid,
    }
}

async fn artist_to_dto(state: &JellyfinState, a: &Artist) -> BaseItemDto {
    let id = mapping::remember_artist(&state.pool, a)
        .await
        .unwrap_or_else(|_| mapping::guid(mapping::KIND_ARTIST, &a.id));
    let user_data = build_user_data(state, &a.id, id.clone()).await;
    // Fetch bio / tags / Last.fm URL when the plugin is enabled. On error
    // (network, quota) we silently fall back to a bare DTO so the artist
    // page still renders.
    let mut overview: Option<String> = None;
    let mut tags: Option<Vec<String>> = None;
    let mut external_urls: Option<Vec<super::dto::ExternalUrl>> = None;
    if let Some(lf) = &state.similar.lastfm {
        if let Ok(info) = lf.artist_info_cached(&state.pool, &a.id, &a.name).await {
            overview = info.bio;
            if !info.tags.is_empty() {
                tags = Some(info.tags);
            }
            if let Some(url) = info.url {
                external_urls = Some(vec![super::dto::ExternalUrl {
                    name: Some("Last.fm".to_string()),
                    url: Some(url),
                }]);
            }
        }
    }
    BaseItemDto {
        id: id.clone(),
        server_id: Some(state.server_id.clone()),
        name: Some(a.name.clone()),
        item_type: "MusicArtist",
        media_type: "Unknown",
        is_folder: Some(true),
        sort_name: Some(a.name.clone()),
        location_type: Some("FileSystem"),
        overview,
        tags,
        external_urls,
        image_tags: Some(ImageTags {
            primary: Some(a.id.clone()),
        }),
        user_data: Some(user_data),
        ..Default::default()
    }
}

async fn album_to_dto(state: &JellyfinState, al: &Album) -> BaseItemDto {
    let id = mapping::remember_album(&state.pool, al)
        .await
        .unwrap_or_else(|_| mapping::guid(mapping::KIND_ALBUM, &al.id));
    let artist_guid = mapping::guid(mapping::KIND_ARTIST, &al.artist_id);
    let song_count = repo::song_count_for_album(&state.pool, &al.id)
        .await
        .unwrap_or(0) as i32;
    let duration_secs = repo::songs_for_album_duration(&state.pool, &al.id)
        .await
        .unwrap_or(0);
    let user_data = build_user_data(state, &al.id, id.clone()).await;
    // Last.fm-sourced Overview / Tags / ExternalUrls, same silent-fallback
    // as `artist_to_dto`.
    let mut overview: Option<String> = None;
    let mut tags: Option<Vec<String>> = None;
    let mut external_urls: Option<Vec<super::dto::ExternalUrl>> = None;
    if let Some(lf) = &state.similar.lastfm {
        if let Ok(info) = lf
            .album_info_cached(&state.pool, &al.id, &al.artist, &al.title)
            .await
        {
            overview = info.bio;
            if !info.tags.is_empty() {
                tags = Some(info.tags);
            }
            if let Some(url) = info.url {
                external_urls = Some(vec![super::dto::ExternalUrl {
                    name: Some("Last.fm".to_string()),
                    url: Some(url),
                }]);
            }
        }
    }
    BaseItemDto {
        id: id.clone(),
        server_id: Some(state.server_id.clone()),
        name: Some(al.title.clone()),
        item_type: "MusicAlbum",
        media_type: "Unknown",
        is_folder: Some(true),
        production_year: if al.year > 0 {
            Some(al.year as i32)
        } else {
            None
        },
        premiere_date: if al.year > 0 {
            Some(format!("{:04}-01-01T00:00:00.0000000", al.year))
        } else {
            None
        },
        album: Some(al.title.clone()),
        album_id: Some(id.clone()),
        album_artist: Some(al.artist.clone()),
        album_artists: Some(vec![NameGuidPair {
            name: Some(al.artist.clone()),
            id: artist_guid.clone(),
        }]),
        artist_items: Some(vec![NameGuidPair {
            name: Some(al.artist.clone()),
            id: artist_guid.clone(),
        }]),
        artists: Some(vec![al.artist.clone()]),
        parent_id: Some(artist_guid),
        sort_name: Some(al.title.clone()),
        run_time_ticks: Some(duration_secs * 1000 * TICKS_PER_MS),
        song_count: Some(song_count),
        child_count: Some(song_count),
        location_type: Some("FileSystem"),
        overview,
        tags,
        external_urls,
        image_tags: Some(ImageTags {
            primary: al.cover_art.clone().map(|_| al.id.clone()),
        }),
        user_data: Some(user_data),
        ..Default::default()
    }
}

async fn song_to_dto(state: &JellyfinState, s: &Song) -> BaseItemDto {
    let id = mapping::remember_song(&state.pool, s)
        .await
        .unwrap_or_else(|_| mapping::guid(mapping::KIND_SONG, &s.id));
    let album_guid = mapping::guid(mapping::KIND_ALBUM, &s.album_id);
    let artist_guid = mapping::guid(mapping::KIND_ARTIST, &s.artist_id);
    let run_time_ticks = s.duration_ms * TICKS_PER_MS;
    let user_data = build_user_data(state, &s.id, id.clone()).await;

    let audio_stream = MediaStream {
        codec: Some(s.suffix.clone()),
        stream_type: "Audio",
        index: 0,
        is_default: true,
        channels: Some(2),
        sample_rate: None,
        bit_rate: Some((s.bitrate * 1000) as i32),
        video_range: "Unknown",
        video_range_type: "Unknown",
        audio_spatial_format: "None",
        is_interlaced: false,
        is_forced: false,
        is_hearing_impaired: false,
        is_original: false,
        is_external: false,
        is_text_subtitle_stream: false,
        supports_external_stream: false,
        ..Default::default()
    };

    let media_source = MediaSource {
        protocol: "File",
        id: Some(id.clone()),
        path: Some(s.path.clone()),
        source_type: "Default",
        container: Some(s.suffix.clone()),
        size: Some(s.filesize),
        name: Some(s.title.clone()),
        is_remote: false,
        run_time_ticks: Some(run_time_ticks),
        read_at_native_framerate: false,
        ignore_dts: false,
        ignore_index: false,
        gen_pts_input: false,
        supports_transcoding: false,
        supports_direct_stream: true,
        supports_direct_play: true,
        is_infinite_stream: false,
        use_most_compatible_transcoding_profile: false,
        requires_opening: false,
        requires_closing: false,
        requires_looping: false,
        supports_probing: false,
        media_streams: Some(vec![audio_stream.clone()]),
        formats: None,
        bitrate: Some((s.bitrate * 1000) as i32),
        transcoding_sub_protocol: "http",
        default_audio_stream_index: Some(0),
        default_subtitle_stream_index: None,
        has_segments: false,
    };

    BaseItemDto {
        id: id.clone(),
        server_id: Some(state.server_id.clone()),
        name: Some(s.title.clone()),
        item_type: "Audio",
        media_type: "Audio",
        is_folder: Some(false),
        production_year: s.year.map(|y| y as i32),
        index_number: s.track_number.map(|n| n as i32),
        parent_index_number: s.disc_number.map(|n| n as i32),
        run_time_ticks: Some(run_time_ticks),
        container: Some(s.suffix.clone()),
        path: Some(s.path.clone()),
        album: Some(s.album.clone()),
        album_id: Some(album_guid.clone()),
        album_artist: Some(s.artist.clone()),
        album_artists: Some(vec![NameGuidPair {
            name: Some(s.artist.clone()),
            id: artist_guid.clone(),
        }]),
        artist_items: Some(vec![NameGuidPair {
            name: Some(s.artist.clone()),
            id: artist_guid.clone(),
        }]),
        artists: Some(vec![s.artist.clone()]),
        parent_id: Some(album_guid),
        genres: s.genre.as_ref().map(|g| vec![g.clone()]),
        location_type: Some("FileSystem"),
        media_sources: Some(vec![media_source.clone()]),
        media_source_count: Some(1),
        media_streams: Some(vec![audio_stream]),
        image_tags: Some(ImageTags {
            primary: s.cover_art.clone().or_else(|| Some(s.album_id.clone())),
        }),
        image_blur_hashes: Some(ImageBlurHashes::default()),
        user_data: Some(user_data),
        ..Default::default()
    }
}

async fn playlist_to_dto(state: &JellyfinState, p: &Playlist) -> BaseItemDto {
    let id = mapping::remember_playlist(&state.pool, p)
        .await
        .unwrap_or_else(|_| mapping::guid(mapping::KIND_PLAYLIST, &p.id));
    let songs = repo::playlist_songs(&state.pool, &p.id)
        .await
        .unwrap_or_default();
    let child_count = songs.len() as i32;
    let run_time_ticks: i64 = songs.iter().map(|s| s.duration_ms * TICKS_PER_MS).sum();
    let user_data = build_user_data(state, &p.id, id.clone()).await;
    BaseItemDto {
        id: id.clone(),
        server_id: Some(state.server_id.clone()),
        name: Some(p.name.clone()),
        item_type: "Playlist",
        media_type: "Audio",
        is_folder: Some(true),
        collection_type: Some("playlist"),
        location_type: Some("FileSystem"),
        sort_name: Some(p.name.clone()),
        date_created: Some(p.created_at.clone()),
        overview: p.comment.clone(),
        child_count: Some(child_count),
        song_count: Some(child_count),
        run_time_ticks: Some(run_time_ticks),
        user_data: Some(user_data),
        ..Default::default()
    }
}

async fn video_to_dto(state: &JellyfinState, v: &Video) -> BaseItemDto {
    let id = mapping::remember_video(&state.pool, v)
        .await
        .unwrap_or_else(|_| mapping::guid(mapping::KIND_VIDEO, &v.id));
    let run_time_ticks = v.duration_ms * TICKS_PER_MS;
    let user_data = build_user_data(state, &v.id, id.clone()).await;

    let video_stream = MediaStream {
        codec: Some(v.container.clone()),
        stream_type: "Video",
        index: 0,
        is_default: true,
        channels: None,
        sample_rate: None,
        bit_rate: Some(v.bitrate as i32),
        height: if v.height > 0 {
            Some(v.height as i32)
        } else {
            None
        },
        width: if v.width > 0 {
            Some(v.width as i32)
        } else {
            None
        },
        video_range: "SDR",
        video_range_type: "SDR",
        audio_spatial_format: "None",
        is_interlaced: false,
        is_forced: false,
        is_hearing_impaired: false,
        is_original: false,
        is_external: false,
        is_text_subtitle_stream: false,
        supports_external_stream: false,
        ..Default::default()
    };

    let media_source = MediaSource {
        protocol: "File",
        id: Some(id.clone()),
        path: Some(v.path.clone()),
        source_type: "Default",
        container: Some(v.container.clone()),
        size: Some(v.filesize),
        name: Some(v.title.clone()),
        is_remote: false,
        run_time_ticks: Some(run_time_ticks),
        read_at_native_framerate: false,
        ignore_dts: false,
        ignore_index: false,
        gen_pts_input: false,
        supports_transcoding: false,
        supports_direct_stream: true,
        supports_direct_play: true,
        is_infinite_stream: false,
        use_most_compatible_transcoding_profile: false,
        requires_opening: false,
        requires_closing: false,
        requires_looping: false,
        supports_probing: true,
        media_streams: Some(vec![video_stream.clone()]),
        formats: None,
        bitrate: Some(v.bitrate as i32),
        transcoding_sub_protocol: "http",
        default_audio_stream_index: None,
        default_subtitle_stream_index: None,
        has_segments: false,
    };

    BaseItemDto {
        id: id.clone(),
        server_id: Some(state.server_id.clone()),
        name: Some(v.title.clone()),
        item_type: "Movie",
        media_type: "Video",
        is_folder: Some(false),
        run_time_ticks: Some(run_time_ticks),
        container: Some(v.container.clone()),
        path: Some(v.path.clone()),
        parent_id: Some(mapping::movies_library_guid()),
        sort_name: Some(v.title.clone()),
        height: if v.height > 0 {
            Some(v.height as i32)
        } else {
            None
        },
        width: if v.width > 0 {
            Some(v.width as i32)
        } else {
            None
        },
        location_type: Some("FileSystem"),
        media_sources: Some(vec![media_source.clone()]),
        media_source_count: Some(1),
        media_streams: Some(vec![video_stream]),
        image_tags: Some(ImageTags {
            primary: v.poster_path.as_ref().map(|_| id.clone()),
        }),
        image_blur_hashes: Some(ImageBlurHashes::default()),
        user_data: Some(user_data),
        ..Default::default()
    }
}

fn includes(types_csv: &Option<String>, want: &str) -> bool {
    match types_csv {
        None => false,
        Some(s) => s.split(',').any(|p| p.trim().eq_ignore_ascii_case(want)),
    }
}

/// Clients ask for the global video list in three different ways:
/// `IncludeItemTypes=Movie` (Jellyfin web/Findroid), `IncludeItemTypes=Video`
/// (some SDK consumers using the BaseItemKind enum), and `MediaTypes=Video`
/// (clients filtering purely by media type). Treat all three as the same
/// request so smolsonic actually returns videos instead of an artist list.
fn wants_videos(q: &ItemsQuery) -> bool {
    includes(&q.include_item_types, "Movie")
        || includes(&q.include_item_types, "Video")
        || includes(&q.media_types, "Video")
}

async fn resolve_native(state: &JellyfinState, guid: &str) -> Option<(String, String)> {
    mapping::lookup(&state.pool, guid).await.ok().flatten()
}

pub async fn items(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    items_impl(state, parse_items_query(&req)).await
}

pub async fn user_items(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    _path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    items_impl(state, parse_items_query(&req)).await
}

async fn items_impl(state: web::Data<JellyfinState>, q: ItemsQuery) -> HttpResponse {
    let library_id = mapping::library_guid();
    let movies_id = mapping::movies_library_guid();
    let playlists_id = mapping::playlists_library_guid();

    // Favorites-only listing — `?Filters=IsFavorite` or `?IsFavorite=true`,
    // matching the Jellyfin OpenAPI spec's two ways of asking. This runs
    // before *any* other dispatch (including the library-views fallback)
    // because the reference client's "Favorites" home rail sends
    // `?Filters=IsFavorite&Recursive=true` with no ParentId — without this
    // short-circuit we'd return CollectionFolder tiles instead.
    if wants_favorites_only(&q) {
        return list_favorites(&state, &q).await;
    }

    // `/Items?userId=X` with no other filters is the "show me my libraries"
    // call (real Jellyfin returns CollectionFolders here). Findroid's
    // MediaViewModel uses this — without this branch we'd return the artist
    // list, which Findroid then renders as a wall of empty tiles.
    if q.parent_id.is_none()
        && q.ids.is_none()
        && q.search_term.is_none()
        && q.include_item_types.is_none()
        && q.media_types.is_none()
        && q.album_artist_ids.is_none()
        && q.artist_ids.is_none()
        && q.album_ids.is_none()
    {
        let views = all_library_views(&state);
        let total = views.len() as i32;
        return HttpResponse::Ok().json(ItemsResult {
            items: views,
            total_record_count: total,
            start_index: 0,
        });
    }

    // Free-text search (`/Items?searchTerm=...`). Findroid restricts to
    // `Movie`/`Series` types and expects a video result; Finamp restricts to
    // `MusicArtist`/`MusicAlbum`/`Audio`. We honour whichever the client
    // requested; an unfiltered call searches everything.
    if let Some(term) = q.search_term.as_deref().filter(|s| !s.is_empty()) {
        let limit = q.limit.unwrap_or(50).max(1);
        let no_filter = q.include_item_types.is_none();
        let mut dtos: Vec<BaseItemDto> = Vec::new();
        if no_filter || includes(&q.include_item_types, "MusicArtist") {
            if let Ok(rows) = repo::search_artists(&state.pool, term, limit, 0).await {
                for a in rows {
                    dtos.push(artist_to_dto(&state, &a).await);
                }
            }
        }
        if no_filter || includes(&q.include_item_types, "MusicAlbum") {
            if let Ok(rows) = repo::search_albums(&state.pool, term, limit, 0).await {
                for a in rows {
                    dtos.push(album_to_dto(&state, &a).await);
                }
            }
        }
        if no_filter || includes(&q.include_item_types, "Audio") {
            if let Ok(rows) = repo::search_songs(&state.pool, term, limit, 0).await {
                for s in rows {
                    dtos.push(song_to_dto(&state, &s).await);
                }
            }
        }
        if no_filter || wants_videos(&q) {
            if let Ok(rows) = repo::search_videos(&state.pool, term, limit, 0).await {
                for v in rows {
                    dtos.push(video_to_dto(&state, &v).await);
                }
            }
        }
        // "Series"/"Episode" requested → nothing to return; smolsonic has no
        // series concept, so we just don't add anything. Returning an empty
        // result for an unsupported type is correct per spec.
        let total = dtos.len() as i32;
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: 0,
        });
    }

    // Explicit Ids= lookup wins (single or comma-separated).
    if let Some(ids) = q.ids.as_ref() {
        let mut out: Vec<BaseItemDto> = Vec::new();
        for raw in ids.split(',') {
            let g = mapping::normalize_guid(raw.trim());
            if let Some((kind, native)) = resolve_native(&state, &g).await {
                match kind.as_str() {
                    "song" => {
                        if let Ok(Some(s)) = repo::find_song(&state.pool, &native).await {
                            out.push(song_to_dto(&state, &s).await);
                        }
                    }
                    "album" => {
                        if let Ok(Some(a)) = repo::find_album(&state.pool, &native).await {
                            out.push(album_to_dto(&state, &a).await);
                        }
                    }
                    "artist" => {
                        if let Ok(Some(a)) = repo::find_artist(&state.pool, &native).await {
                            out.push(artist_to_dto(&state, &a).await);
                        }
                    }
                    "video" => {
                        if let Ok(Some(v)) = repo::find_video(&state.pool, &native).await {
                            out.push(video_to_dto(&state, &v).await);
                        }
                    }
                    "playlist" => {
                        if let Ok(Some(p)) = repo::find_playlist(&state.pool, &native).await {
                            out.push(playlist_to_dto(&state, &p).await);
                        }
                    }
                    _ => {}
                }
            }
        }
        let total = out.len() as i32;
        return HttpResponse::Ok().json(ItemsResult {
            items: out,
            total_record_count: total,
            start_index: 0,
        });
    }

    // Filter by parent FIRST — even when include_item_types includes "Movie",
    // a parent restricts the scope. Without this, a Findroid click on an
    // artist (with `&includeItemTypes=Folder&includeItemTypes=Movie&includeItemTypes=Series`)
    // would return every movie in the library instead of "no Movie children
    // of this artist".
    if let Some(parent) = q.parent_id.as_ref() {
        let g = mapping::normalize_guid(parent);
        if g == library_id {
            // Top-level inside the music library — list artists by default, albums if requested.
            return list_artists_or_albums(&state, &q).await;
        }
        if g == playlists_id {
            return list_playlists(&state, &q).await;
        }
        if g == movies_id {
            return list_movies(&state, &q).await;
        }
        if let Some((kind, native)) = resolve_native(&state, &g).await {
            match kind.as_str() {
                "artist" => {
                    let albums = repo::albums_by_artist(&state.pool, &native)
                        .await
                        .unwrap_or_default();
                    let mut dtos = Vec::with_capacity(albums.len());
                    for a in &albums {
                        dtos.push(album_to_dto(&state, a).await);
                    }
                    let total = dtos.len() as i32;
                    return HttpResponse::Ok().json(ItemsResult {
                        items: dtos,
                        total_record_count: total,
                        start_index: 0,
                    });
                }
                "album" => {
                    let songs = repo::songs_by_album(&state.pool, &native)
                        .await
                        .unwrap_or_default();
                    let mut dtos = Vec::with_capacity(songs.len());
                    for s in &songs {
                        dtos.push(song_to_dto(&state, s).await);
                    }
                    let total = dtos.len() as i32;
                    return HttpResponse::Ok().json(ItemsResult {
                        items: dtos,
                        total_record_count: total,
                        start_index: 0,
                    });
                }
                "genre" => {
                    // Drill-down from `/Genres` tap. `native` is the exact-
                    // cased genre name (we stored it in `jf_guids.native_id`).
                    let limit = q.limit.unwrap_or(500).max(1);
                    let offset = q.start_index.unwrap_or(0).max(0);
                    let songs = repo::songs_by_genre(&state.pool, &native, limit, offset)
                        .await
                        .unwrap_or_default();
                    // If the client asked specifically for albums, dedupe
                    // song rows down to their albums.
                    if includes(&q.include_item_types, "MusicAlbum") {
                        let mut seen: std::collections::HashSet<String> =
                            std::collections::HashSet::new();
                        let mut dtos: Vec<BaseItemDto> = Vec::new();
                        for s in &songs {
                            if seen.insert(s.album_id.clone()) {
                                if let Ok(Some(al)) =
                                    repo::find_album(&state.pool, &s.album_id).await
                                {
                                    dtos.push(album_to_dto(&state, &al).await);
                                }
                            }
                        }
                        let total = dtos.len() as i32;
                        return HttpResponse::Ok().json(ItemsResult {
                            items: dtos,
                            total_record_count: total,
                            start_index: offset as i32,
                        });
                    }
                    let mut dtos = Vec::with_capacity(songs.len());
                    for s in &songs {
                        dtos.push(song_to_dto(&state, s).await);
                    }
                    let total = dtos.len() as i32;
                    return HttpResponse::Ok().json(ItemsResult {
                        items: dtos,
                        total_record_count: total,
                        start_index: offset as i32,
                    });
                }
                "year" => {
                    // Drill-down from `/Years` tap. `native` is the year
                    // string (we stored it in `jf_guids.native_id`).
                    let Ok(year) = native.parse::<i32>() else {
                        return HttpResponse::NotFound().finish();
                    };
                    // Albums variant — Findroid's "albums by year" tile.
                    if includes(&q.include_item_types, "MusicAlbum") {
                        let albums = repo::albums_by_year(&state.pool, year)
                            .await
                            .unwrap_or_default();
                        let mut dtos = Vec::with_capacity(albums.len());
                        for a in &albums {
                            dtos.push(album_to_dto(&state, a).await);
                        }
                        let total = dtos.len() as i32;
                        return HttpResponse::Ok().json(ItemsResult {
                            items: dtos,
                            total_record_count: total,
                            start_index: 0,
                        });
                    }
                    let limit = q.limit.unwrap_or(500).max(1);
                    let offset = q.start_index.unwrap_or(0).max(0);
                    let songs = repo::songs_by_year(&state.pool, year, limit, offset)
                        .await
                        .unwrap_or_default();
                    let mut dtos = Vec::with_capacity(songs.len());
                    for s in &songs {
                        dtos.push(song_to_dto(&state, s).await);
                    }
                    let total = dtos.len() as i32;
                    return HttpResponse::Ok().json(ItemsResult {
                        items: dtos,
                        total_record_count: total,
                        start_index: offset as i32,
                    });
                }
                _ => {}
            }
        }
        // parent_id was set but didn't resolve to anything meaningful
        // (e.g. a Findroid query for /Items?parentId=<artist>&includeItemTypes=Folder|Movie|Series
        // — none of those types exist inside a MusicArtist). Returning empty
        // is correct; falling through would dump the whole artist list.
        return HttpResponse::Ok().json(ItemsResult {
            items: vec![],
            total_record_count: 0,
            start_index: 0,
        });
    }

    // No parent: filter by item type. Global movie list for any of
    // `?IncludeItemTypes=Movie`, `?IncludeItemTypes=Video`, or `?MediaTypes=Video`.
    if wants_videos(&q) {
        return list_movies(&state, &q).await;
    }

    // Playlists as items — `?includeItemTypes=Playlist` (with or without
    // `Recursive=true`). Real Jellyfin surfaces user-owned playlists through
    // the general Items query as well as via `/Playlists`.
    if includes(&q.include_item_types, "Playlist") {
        return list_playlists(&state, &q).await;
    }

    // No parent: filter by AlbumArtistIds / ArtistIds (album or song
    // browsing for an artist — Amcfy's "artist detail" page does this).
    let artist_filter = q.album_artist_ids.clone().or_else(|| q.artist_ids.clone());

    // Songs by artist — Amcfy/Symfonium: `?artistIds=<X>&includeItemTypes=Audio`.
    if includes(&q.include_item_types, "Audio") {
        if let Some(filter) = artist_filter.clone() {
            if let Some(first) = filter.split(',').next() {
                let g = mapping::normalize_guid(first);
                if let Some((kind, native)) = resolve_native(&state, &g).await {
                    if kind == "artist" {
                        let limit = q.limit.unwrap_or(500).max(1);
                        let offset = q.start_index.unwrap_or(0).max(0);
                        let songs = repo::songs_by_artist(&state.pool, &native, limit, offset)
                            .await
                            .unwrap_or_default();
                        let mut dtos = Vec::with_capacity(songs.len());
                        for s in &songs {
                            dtos.push(song_to_dto(&state, s).await);
                        }
                        let total = dtos.len() as i32;
                        return HttpResponse::Ok().json(ItemsResult {
                            items: dtos,
                            total_record_count: total,
                            start_index: offset as i32,
                        });
                    }
                }
            }
        }
    }

    // Plain "all songs" — `?includeItemTypes=Audio` with no artist/parent.
    // Honours `NameStartsWith` etc. for Finamp's alpha-jump rail.
    if includes(&q.include_item_types, "Audio") {
        let limit = q.limit.unwrap_or(100).max(1);
        let offset = q.start_index.unwrap_or(0).max(0);
        let starts = q.name_starts_with.as_deref();
        let geq = q.name_starts_with_or_greater.as_deref();
        let lt = q.name_less_than.as_deref();
        let songs = repo::songs_filtered(&state.pool, starts, geq, lt, limit, offset)
            .await
            .unwrap_or_default();
        let total = repo::count_songs_filtered(&state.pool, starts, geq, lt)
            .await
            .unwrap_or(0) as i32;
        let mut dtos = Vec::with_capacity(songs.len());
        for s in &songs {
            dtos.push(song_to_dto(&state, s).await);
        }
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: offset as i32,
        });
    }

    if includes(&q.include_item_types, "MusicAlbum") {
        if let Some(filter) = artist_filter {
            if let Some(first) = filter.split(',').next() {
                let g = mapping::normalize_guid(first);
                if let Some((kind, native)) = resolve_native(&state, &g).await {
                    if kind == "artist" {
                        let albums = repo::albums_by_artist(&state.pool, &native)
                            .await
                            .unwrap_or_default();
                        let mut dtos = Vec::with_capacity(albums.len());
                        for a in &albums {
                            dtos.push(album_to_dto(&state, a).await);
                        }
                        let total = dtos.len() as i32;
                        return HttpResponse::Ok().json(ItemsResult {
                            items: dtos,
                            total_record_count: total,
                            start_index: 0,
                        });
                    }
                }
            }
        }
        // No artist filter → paginated all-albums, honouring NameStartsWith.
        let limit = q.limit.unwrap_or(100).max(1);
        let offset = q.start_index.unwrap_or(0).max(0);
        let starts = q.name_starts_with.as_deref();
        let geq = q.name_starts_with_or_greater.as_deref();
        let lt = q.name_less_than.as_deref();
        let albums = repo::albums_filtered(&state.pool, starts, geq, lt, limit, offset)
            .await
            .unwrap_or_default();
        let total = repo::count_albums_filtered(&state.pool, starts, geq, lt)
            .await
            .unwrap_or(0) as i32;
        let mut dtos = Vec::with_capacity(albums.len());
        for a in &albums {
            dtos.push(album_to_dto(&state, a).await);
        }
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: offset as i32,
        });
    }

    list_artists_or_albums(&state, &q).await
}

async fn list_movies(state: &JellyfinState, q: &ItemsQuery) -> HttpResponse {
    let limit = q.limit.unwrap_or(100).max(1);
    let offset = q.start_index.unwrap_or(0).max(0);
    let starts = q.name_starts_with.as_deref();
    let geq = q.name_starts_with_or_greater.as_deref();
    let lt = q.name_less_than.as_deref();
    let videos = repo::videos_filtered(&state.pool, starts, geq, lt, limit, offset)
        .await
        .unwrap_or_default();
    let total = repo::count_videos_filtered(&state.pool, starts, geq, lt)
        .await
        .unwrap_or_default();
    let mut dtos = Vec::with_capacity(videos.len());
    for v in &videos {
        dtos.push(video_to_dto(state, v).await);
    }
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total as i32,
        start_index: offset as i32,
    })
}

/// `/Items?Filters=IsFavorite&IncludeItemTypes=…` — favourites-only listing.
///
/// If no type filter is set the response mixes artists, albums, songs, videos
/// and playlists (that's what the reference Jellyfin server does for the
/// home-screen Favorites rail). When a client narrows via `IncludeItemTypes`
/// we return just that slice. Ordering follows `starred.starred_at DESC` —
/// most-recently-favorited first — which matches how the reference client
/// renders the rail.
async fn list_favorites(state: &JellyfinState, q: &ItemsQuery) -> HttpResponse {
    let type_filter = q.include_item_types.as_deref();
    let all_types = type_filter.is_none();

    let want_artist = all_types
        || includes(&q.include_item_types, "MusicArtist")
        || includes(&q.include_item_types, "AlbumArtist");
    let want_album = all_types || includes(&q.include_item_types, "MusicAlbum");
    let want_song = all_types || includes(&q.include_item_types, "Audio");
    let want_video = all_types || wants_videos(q);
    let want_playlist = all_types || includes(&q.include_item_types, "Playlist");

    let mut dtos: Vec<BaseItemDto> = Vec::new();

    if want_artist {
        if let Ok(rows) = repo::starred_artists(&state.pool).await {
            for (a, _when) in rows {
                dtos.push(artist_to_dto(state, &a).await);
            }
        }
    }
    if want_album {
        if let Ok(rows) = repo::starred_albums(&state.pool).await {
            for (a, _when) in rows {
                dtos.push(album_to_dto(state, &a).await);
            }
        }
    }
    if want_song {
        if let Ok(rows) = repo::starred_songs(&state.pool).await {
            for (s, _when) in rows {
                dtos.push(song_to_dto(state, &s).await);
            }
        }
    }
    if want_video {
        if let Ok(rows) = repo::starred_videos(&state.pool).await {
            for (v, _when) in rows {
                dtos.push(video_to_dto(state, &v).await);
            }
        }
    }
    if want_playlist {
        if let Ok(rows) = repo::starred_playlists(&state.pool).await {
            for (p, _when) in rows {
                dtos.push(playlist_to_dto(state, &p).await);
            }
        }
    }

    let total = dtos.len() as i32;
    // Honour `StartIndex` / `Limit` after the union — the caller sees a single
    // paginated list, not per-type slices.
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let limit = q.limit.unwrap_or(500).max(1) as usize;
    let items: Vec<BaseItemDto> = dtos.into_iter().skip(start).take(limit).collect();
    HttpResponse::Ok().json(ItemsResult {
        items,
        total_record_count: total,
        start_index: start as i32,
    })
}

// ── Playlists ────────────────────────────────────────────────────────────────

/// Filter `playlists` in-memory by `NameStartsWith` / `NameStartsWithOrGreater`
/// / `NameLessThan`. Playlists live in a small table so the extra pass is
/// cheap and keeps the repo API flat.
fn filter_playlists_by_query(playlists: Vec<Playlist>, q: &ItemsQuery) -> Vec<Playlist> {
    playlists
        .into_iter()
        .filter(|p| {
            let name = p.name.to_lowercase();
            if let Some(s) = q.name_starts_with.as_deref() {
                if !name.starts_with(&s.to_lowercase()) {
                    return false;
                }
            }
            if let Some(s) = q.name_starts_with_or_greater.as_deref() {
                if name.as_str() < s.to_lowercase().as_str() {
                    return false;
                }
            }
            if let Some(s) = q.name_less_than.as_deref() {
                if name.as_str() >= s.to_lowercase().as_str() {
                    return false;
                }
            }
            true
        })
        .collect()
}

async fn list_playlists(state: &JellyfinState, q: &ItemsQuery) -> HttpResponse {
    let playlists = repo::all_playlists(&state.pool).await.unwrap_or_default();
    let playlists = filter_playlists_by_query(playlists, q);
    let total = playlists.len() as i32;
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let limit = q.limit.unwrap_or(500).max(1) as usize;
    let slice: Vec<&Playlist> = playlists.iter().skip(start).take(limit).collect();
    let mut dtos = Vec::with_capacity(slice.len());
    for p in slice {
        dtos.push(playlist_to_dto(state, p).await);
    }
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: start as i32,
    })
}

/// Convert a `PlaylistItemId` (our synthesized entry GUID) back to a
/// 0-based position inside the playlist. Falls back to accepting a raw
/// integer position or a plain song GUID.
async fn entry_id_to_position(
    state: &JellyfinState,
    playlist_native: &str,
    entry_id: &str,
) -> Option<i64> {
    let normalized = mapping::normalize_guid(entry_id);

    let song_ids = repo::playlist_song_ids(&state.pool, playlist_native)
        .await
        .ok()?;
    for (pos, sid) in song_ids.iter().enumerate() {
        let expected = mapping::playlist_entry_guid(playlist_native, pos as i64);
        if mapping::normalize_guid(&expected) == normalized {
            return Some(pos as i64);
        }
        // Some clients pass the song's own GUID as the entry id.
        let song_guid = mapping::guid(mapping::KIND_SONG, sid);
        if mapping::normalize_guid(&song_guid) == normalized {
            return Some(pos as i64);
        }
    }
    entry_id.parse::<i64>().ok()
}

/// Resolve a comma-separated list of song GUIDs to native song ids. Skips
/// GUIDs that don't map to a song so callers can silently ignore garbage.
async fn resolve_song_native_ids(state: &JellyfinState, ids_csv: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in ids_csv.split(',') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let g = mapping::normalize_guid(trimmed);
        if let Some((kind, native)) = resolve_native(state, &g).await {
            if kind == "song" {
                out.push(native);
            }
        }
    }
    out
}

fn new_playlist_native_id() -> String {
    format!("pl-{}", auth::random_hex(8))
}

/// `GET /Playlists` — some clients probe this to enumerate playlists in the
/// same shape as `/Items`. Real Jellyfin has no such endpoint (playlists are
/// surfaced through `/Items?IncludeItemTypes=Playlist`), but returning the
/// list here keeps 3rd-party clients happy.
pub async fn playlists_list(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let q = parse_items_query(&req);
    list_playlists(&state, &q).await
}

/// `POST /Playlists` — `CreatePlaylistDto` body OR query params. Response is
/// `PlaylistCreationResult { Id }`.
pub async fn create_playlist_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
    body: Option<web::Json<Value>>,
) -> HttpResponse {
    let query = collect_query(&req);
    let q_one = |k: &str| {
        query
            .get(k)
            .and_then(|v| v.first())
            .cloned()
            .filter(|s| !s.is_empty())
    };
    let q_ids = || -> Vec<String> {
        query
            .get("ids")
            .or_else(|| query.get("Ids"))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .filter(|s| !s.is_empty())
            .collect()
    };

    let body_val = body.map(|b| b.into_inner());

    let name = body_val
        .as_ref()
        .and_then(|v| v.get("Name").and_then(|n| n.as_str()).map(String::from))
        .or_else(|| q_one("name"))
        .or_else(|| q_one("Name"))
        .unwrap_or_else(|| "New Playlist".to_string());

    let body_ids: Vec<String> = body_val
        .as_ref()
        .and_then(|v| v.get("Ids").and_then(|a| a.as_array()))
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let all_ids: Vec<String> = if body_ids.is_empty() {
        q_ids()
    } else {
        body_ids
    };
    let song_ids = resolve_song_native_ids(&state, &all_ids.join(",")).await;

    let native = new_playlist_native_id();
    let now = now_iso();
    if let Err(e) = repo::create_playlist(&state.pool, &native, &name, &now).await {
        tracing::error!("jellyfin: create_playlist: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    if !song_ids.is_empty() {
        if let Err(e) = repo::append_playlist_songs(&state.pool, &native, &song_ids).await {
            tracing::error!("jellyfin: append_playlist_songs: {e}");
        }
    }
    let guid = mapping::guid(mapping::KIND_PLAYLIST, &native);
    // Best-effort — the id is deterministic so a failed remember() just costs
    // us one lookup on the next request.
    let _ = mapping::remember(&state.pool, mapping::KIND_PLAYLIST, &native).await;
    HttpResponse::Ok().json(PlaylistCreationResult { id: guid })
}

/// `GET /Playlists/{id}` — playlist metadata as a `BaseItemDto`.
pub async fn get_playlist_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    match repo::find_playlist(&state.pool, &native).await {
        Ok(Some(p)) => HttpResponse::Ok().json(playlist_to_dto(&state, &p).await),
        _ => HttpResponse::NotFound().finish(),
    }
}

/// `POST /Playlists/{id}` — `UpdatePlaylistDto` body. Supports rename and
/// full-replace of the item list; other fields (Users, IsPublic) are
/// accepted and ignored because smolsonic has a single-user model.
pub async fn update_playlist_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    body: Option<web::Json<Value>>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    let body = match body {
        Some(b) => b.into_inner(),
        None => return HttpResponse::NoContent().finish(),
    };
    let now = now_iso();
    if let Some(name) = body.get("Name").and_then(|n| n.as_str()) {
        if !name.is_empty() {
            let _ = repo::rename_playlist(&state.pool, &native, name, &now).await;
        }
    }
    if let Some(ids) = body.get("Ids").and_then(|a| a.as_array()) {
        let csv: String = ids
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let song_ids = resolve_song_native_ids(&state, &csv).await;
        let _ = repo::replace_playlist_songs(&state.pool, &native, &song_ids).await;
    }
    HttpResponse::NoContent().finish()
}

/// `GET /Playlists/{id}/Items` — `BaseItemDtoQueryResult` of the playlist's
/// songs, in order. Each song DTO carries a `PlaylistItemId` so clients can
/// reference specific entries for remove/move.
pub async fn playlist_items(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    let songs = repo::playlist_songs(&state.pool, &native)
        .await
        .unwrap_or_default();
    let total = songs.len() as i32;
    let q = parse_items_query(&req);
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let limit = q.limit.unwrap_or(500).max(1) as usize;

    let mut dtos: Vec<BaseItemDto> = Vec::new();
    for (pos, s) in songs.iter().enumerate().skip(start).take(limit) {
        let mut dto = song_to_dto(&state, s).await;
        dto.playlist_item_id = Some(mapping::playlist_entry_guid(&native, pos as i64));
        dtos.push(dto);
    }
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: start as i32,
    })
}

/// `POST /Playlists/{id}/Items?ids=...` — append songs at the end.
pub async fn add_playlist_items(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    let query = collect_query(&req);
    let ids_csv = query
        .get("ids")
        .or_else(|| query.get("Ids"))
        .cloned()
        .unwrap_or_default()
        .join(",");
    let song_ids = resolve_song_native_ids(&state, &ids_csv).await;
    if !song_ids.is_empty() {
        let _ = repo::append_playlist_songs(&state.pool, &native, &song_ids).await;
        if let Ok(Some(p)) = repo::find_playlist(&state.pool, &native).await {
            let _ = repo::rename_playlist(&state.pool, &native, &p.name, &now_iso()).await;
        }
    }
    HttpResponse::NoContent().finish()
}

/// `DELETE /Playlists/{id}/Items?entryIds=...` — remove one or more
/// entries, identified by `PlaylistItemId` (or as a fallback, the raw song
/// GUID or 0-based position).
pub async fn remove_playlist_items(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    let query = collect_query(&req);
    let entry_ids: Vec<String> = query
        .get("entryIds")
        .or_else(|| query.get("EntryIds"))
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|s| !s.is_empty())
        .collect();

    if entry_ids.is_empty() {
        return HttpResponse::NoContent().finish();
    }

    let current = repo::playlist_song_ids(&state.pool, &native)
        .await
        .unwrap_or_default();
    let mut positions_to_remove: std::collections::BTreeSet<i64> =
        std::collections::BTreeSet::new();
    for eid in &entry_ids {
        if let Some(pos) = entry_id_to_position(&state, &native, eid).await {
            positions_to_remove.insert(pos);
        }
    }
    let kept: Vec<String> = current
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !positions_to_remove.contains(&(*i as i64)))
        .map(|(_, sid)| sid)
        .collect();
    if let Err(e) = repo::replace_playlist_songs(&state.pool, &native, &kept).await {
        tracing::error!("jellyfin: remove_playlist_items: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    let now = now_iso();
    if let Ok(Some(p)) = repo::find_playlist(&state.pool, &native).await {
        let _ = repo::rename_playlist(&state.pool, &native, &p.name, &now).await;
    }
    HttpResponse::NoContent().finish()
}

/// `POST /Playlists/{id}/Items/{itemId}/Move/{newIndex}` — move `itemId`
/// (a `PlaylistItemId`) to `newIndex`, shifting the neighbours.
pub async fn move_playlist_item(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String, i64)>,
) -> HttpResponse {
    let (playlist_id, item_id, new_index) = path.into_inner();
    let g = mapping::normalize_guid(&playlist_id);
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    let Some(from) = entry_id_to_position(&state, &native, &item_id).await else {
        return HttpResponse::NotFound().finish();
    };
    let mut current = repo::playlist_song_ids(&state.pool, &native)
        .await
        .unwrap_or_default();
    if from < 0 || (from as usize) >= current.len() {
        return HttpResponse::BadRequest().finish();
    }
    let target = new_index.max(0).min(current.len() as i64 - 1) as usize;
    let song = current.remove(from as usize);
    current.insert(target, song);
    if let Err(e) = repo::replace_playlist_songs(&state.pool, &native, &current).await {
        tracing::error!("jellyfin: move_playlist_item: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    let now = now_iso();
    if let Ok(Some(p)) = repo::find_playlist(&state.pool, &native).await {
        let _ = repo::rename_playlist(&state.pool, &native, &p.name, &now).await;
    }
    HttpResponse::NoContent().finish()
}

/// `GET /Playlists/{id}/Users` — smolsonic is single-user, so this is always
/// an empty list. Clients that check permissions before showing the "add
/// user" UI silently hide it when this is empty, which is the desired
/// behaviour here.
pub async fn playlist_users(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, _native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    HttpResponse::Ok().json(Vec::<Value>::new())
}

/// `DELETE /Items/{id}` — Jellyfin uses this to delete playlists (playlists
/// ARE items). We only allow it for playlists; deleting songs/albums via the
/// API is not supported.
pub async fn delete_item(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::Forbidden().finish();
    }
    if let Err(e) = repo::delete_playlist(&state.pool, &native).await {
        tracing::error!("jellyfin: delete_playlist: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    HttpResponse::NoContent().finish()
}

/// `/Artists/Prefixes` — same shape as `/Items/Prefixes?IncludeItemTypes=MusicArtist`,
/// but the URL itself implies the type so we don't need query-string hints.
pub async fn artists_prefixes(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    let letters = repo::artist_name_prefixes(&state.pool)
        .await
        .unwrap_or_default();
    let items: Vec<Value> = letters.into_iter().map(|n| json!({ "Name": n })).collect();
    HttpResponse::Ok().json(items)
}

/// `/Items/Prefixes?ParentId=<lib>&IncludeItemTypes=...` — populates the
/// alpha-jump rail. Returns the distinct uppercase first letters that
/// actually exist for the requested item type, with "#" for non-alpha names.
/// Response shape mirrors the reference Jellyfin server: `[{"Name":"A"}, …]`.
pub async fn items_prefixes(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let q = parse_items_query(&req);
    let parent_kind = q
        .parent_id
        .as_deref()
        .map(mapping::normalize_guid)
        .map(|g| {
            if g == mapping::movies_library_guid() {
                "movies"
            } else if g == mapping::library_guid() {
                "music"
            } else if g == mapping::playlists_library_guid() {
                "playlists"
            } else {
                ""
            }
        })
        .unwrap_or("");

    if parent_kind == "playlists" || includes(&q.include_item_types, "Playlist") {
        let names = repo::all_playlists(&state.pool).await.unwrap_or_default();
        let mut letters: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for p in &names {
            let first = p
                .name
                .chars()
                .next()
                .map(|c| c.to_ascii_uppercase().to_string())
                .unwrap_or_default();
            if let Some(c) = first.chars().next() {
                if c.is_ascii_alphabetic() {
                    letters.insert(first);
                } else {
                    letters.insert("#".to_string());
                }
            }
        }
        let items: Vec<Value> = letters.into_iter().map(|n| json!({ "Name": n })).collect();
        return HttpResponse::Ok().json(items);
    }

    let letters = if wants_videos(&q) || parent_kind == "movies" {
        repo::video_name_prefixes(&state.pool).await
    } else if includes(&q.include_item_types, "MusicAlbum") {
        repo::album_name_prefixes(&state.pool).await
    } else if includes(&q.include_item_types, "Audio") {
        repo::song_name_prefixes(&state.pool).await
    } else if includes(&q.include_item_types, "MusicArtist")
        || includes(&q.include_item_types, "AlbumArtist")
        || parent_kind == "music"
    {
        repo::artist_name_prefixes(&state.pool).await
    } else {
        Ok(Vec::new())
    };

    let items: Vec<Value> = letters
        .unwrap_or_default()
        .into_iter()
        .map(|n| json!({ "Name": n }))
        .collect();
    HttpResponse::Ok().json(items)
}

async fn list_artists_or_albums(state: &JellyfinState, q: &ItemsQuery) -> HttpResponse {
    let limit = q.limit.unwrap_or(100).max(1);
    let offset = q.start_index.unwrap_or(0).max(0);
    let starts = q.name_starts_with.as_deref();
    let geq = q.name_starts_with_or_greater.as_deref();
    let lt = q.name_less_than.as_deref();

    // Songs in the music library — `parentId=<music_lib>&includeItemTypes=Audio`.
    if includes(&q.include_item_types, "Audio") {
        let songs = repo::songs_filtered(&state.pool, starts, geq, lt, limit, offset)
            .await
            .unwrap_or_default();
        let total = repo::count_songs_filtered(&state.pool, starts, geq, lt)
            .await
            .unwrap_or(0) as i32;
        let mut dtos = Vec::with_capacity(songs.len());
        for s in &songs {
            dtos.push(song_to_dto(state, s).await);
        }
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: offset as i32,
        });
    }

    if includes(&q.include_item_types, "MusicAlbum") {
        let albums = repo::albums_filtered(&state.pool, starts, geq, lt, limit, offset)
            .await
            .unwrap_or_default();
        let total = repo::count_albums_filtered(&state.pool, starts, geq, lt)
            .await
            .unwrap_or(0) as i32;
        let mut dtos = Vec::with_capacity(albums.len());
        for a in &albums {
            dtos.push(album_to_dto(state, a).await);
        }
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: offset as i32,
        });
    }
    let artists = repo::artists_filtered(&state.pool, starts, geq, lt, limit, offset)
        .await
        .unwrap_or_default();
    let total = repo::count_artists_filtered(&state.pool, starts, geq, lt)
        .await
        .unwrap_or(0) as i32;
    let mut dtos = Vec::with_capacity(artists.len());
    for a in &artists {
        dtos.push(artist_to_dto(state, a).await);
    }
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: offset as i32,
    })
}

/// If `guid` is one of the virtual library GUIDs, return its CollectionFolder
/// DTO. Moonfin tapping a library tile first calls
/// `GET /Users/{uid}/Items/{library_guid}` for the header; without this we
/// returned 404 and the whole library page failed to render.
fn library_view_for(state: &JellyfinState, guid: &str) -> Option<BaseItemDto> {
    if guid == mapping::library_guid() {
        Some(music_library_view(state))
    } else if guid == mapping::movies_library_guid() {
        movies_library_view(state)
    } else if guid == mapping::playlists_library_guid() {
        Some(playlists_library_view(state))
    } else {
        None
    }
}

pub async fn item_by_id(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    if let Some(view) = library_view_for(&state, &g) {
        return HttpResponse::Ok().json(view);
    }
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    match kind.as_str() {
        "song" => match repo::find_song(&state.pool, &native).await {
            Ok(Some(s)) => HttpResponse::Ok().json(song_to_dto(&state, &s).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "album" => match repo::find_album(&state.pool, &native).await {
            Ok(Some(a)) => HttpResponse::Ok().json(album_to_dto(&state, &a).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "artist" => match repo::find_artist(&state.pool, &native).await {
            Ok(Some(a)) => HttpResponse::Ok().json(artist_to_dto(&state, &a).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "video" => match repo::find_video(&state.pool, &native).await {
            Ok(Some(v)) => HttpResponse::Ok().json(video_to_dto(&state, &v).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "playlist" => match repo::find_playlist(&state.pool, &native).await {
            Ok(Some(p)) => HttpResponse::Ok().json(playlist_to_dto(&state, &p).await),
            _ => HttpResponse::NotFound().finish(),
        },
        _ => HttpResponse::NotFound().finish(),
    }
}

pub async fn user_item_by_id(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    let g = mapping::normalize_guid(&item_id);
    if let Some(view) = library_view_for(&state, &g) {
        return HttpResponse::Ok().json(view);
    }
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    match kind.as_str() {
        "song" => match repo::find_song(&state.pool, &native).await {
            Ok(Some(s)) => HttpResponse::Ok().json(song_to_dto(&state, &s).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "album" => match repo::find_album(&state.pool, &native).await {
            Ok(Some(a)) => HttpResponse::Ok().json(album_to_dto(&state, &a).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "artist" => match repo::find_artist(&state.pool, &native).await {
            Ok(Some(a)) => HttpResponse::Ok().json(artist_to_dto(&state, &a).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "video" => match repo::find_video(&state.pool, &native).await {
            Ok(Some(v)) => HttpResponse::Ok().json(video_to_dto(&state, &v).await),
            _ => HttpResponse::NotFound().finish(),
        },
        "playlist" => match repo::find_playlist(&state.pool, &native).await {
            Ok(Some(p)) => HttpResponse::Ok().json(playlist_to_dto(&state, &p).await),
            _ => HttpResponse::NotFound().finish(),
        },
        _ => HttpResponse::NotFound().finish(),
    }
}

// ── Artists endpoints (some clients prefer these over /Items) ───────────────

pub async fn artists(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let q = parse_items_query(&req);

    // `?isFavorite=true` / `?Filters=IsFavorite` on `/Artists` — return only
    // starred artists, honouring the same start/limit paging as the plain call.
    if wants_favorites_only(&q) {
        let starts = q.name_starts_with.as_deref();
        let rows = repo::starred_artists(&state.pool).await.unwrap_or_default();
        let filtered: Vec<Artist> = rows
            .into_iter()
            .map(|(a, _)| a)
            .filter(|a| match starts {
                Some(p) if !p.is_empty() => a
                    .name
                    .to_ascii_lowercase()
                    .starts_with(&p.to_ascii_lowercase()),
                _ => true,
            })
            .collect();
        let total = filtered.len() as i32;
        let start = q.start_index.unwrap_or(0).max(0) as usize;
        let limit = q.limit.unwrap_or(500).max(1) as usize;
        let slice: Vec<&Artist> = filtered.iter().skip(start).take(limit).collect();
        let mut dtos = Vec::with_capacity(slice.len());
        for a in slice {
            dtos.push(artist_to_dto(&state, a).await);
        }
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: start as i32,
        });
    }

    let limit = q.limit.unwrap_or(500).max(1);
    let offset = q.start_index.unwrap_or(0).max(0);
    let starts = q.name_starts_with.as_deref();
    let geq = q.name_starts_with_or_greater.as_deref();
    let lt = q.name_less_than.as_deref();
    let artists = repo::artists_filtered(&state.pool, starts, geq, lt, limit, offset)
        .await
        .unwrap_or_default();
    let total = repo::count_artists_filtered(&state.pool, starts, geq, lt)
        .await
        .unwrap_or(0) as i32;
    let mut dtos = Vec::with_capacity(artists.len());
    for a in &artists {
        dtos.push(artist_to_dto(&state, a).await);
    }
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: offset as i32,
    })
}

pub async fn artist_by_name(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let name = path.into_inner();
    let artists = repo::all_artists(&state.pool).await.unwrap_or_default();
    if let Some(a) = artists.iter().find(|a| a.name.eq_ignore_ascii_case(&name)) {
        HttpResponse::Ok().json(artist_to_dto(&state, a).await)
    } else {
        HttpResponse::NotFound().finish()
    }
}

// ── InstantMix ──────────────────────────────────────────────────────────────
//
// Spec (Jellyfin OpenAPI, InstantMix tag): seven endpoints — one per seed
// kind — each returning `BaseItemDtoQueryResult` (an `ItemsResult` of
// `Audio` items). Real Jellyfin uses a music-similarity backend; smolsonic
// approximates with a three-tier fallback:
//   1. songs by the seed's artist  (or the seed song itself first)
//   2. songs in the seed's genre
//   3. random library-wide filler
// The result is deduped by native id and capped at `limit` (default 50 in
// the spec — Symfonium/Streamyfin both use 100).

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `id` is accepted for spec-compliance but not consulted.
pub struct InstantMixQuery {
    #[serde(default, alias = "Limit")]
    pub limit: Option<i64>,
    /// Only present on `/MusicGenres/InstantMix` — the seed genre's id.
    /// smolsonic doesn't emit Genre BaseItems so we accept-and-ignore.
    #[serde(default, alias = "Id")]
    pub id: Option<String>,
}

/// Assemble the InstantMix result. `artist_id` narrows tier 1, `genre` narrows
/// tier 2, `seeds` are inserted at the front (used by the song- and
/// playlist-seeded mixes). Any of the tiers can be absent — the next one
/// takes over.
async fn build_instant_mix(
    state: &JellyfinState,
    limit: i64,
    seeds: Vec<Song>,
    artist_id: Option<String>,
    genre: Option<String>,
) -> Vec<Song> {
    let target = limit.max(1) as usize;
    let mut out: Vec<Song> = Vec::with_capacity(target);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for s in seeds {
        if out.len() >= target {
            break;
        }
        if seen.insert(s.id.clone()) {
            out.push(s);
        }
    }

    // Tier 1: same artist.
    if let Some(aid) = artist_id.as_deref() {
        if out.len() < target {
            let more = repo::random_songs_by_artist(&state.pool, aid, target as i64 * 2)
                .await
                .unwrap_or_default();
            for s in more {
                if out.len() >= target {
                    break;
                }
                if seen.insert(s.id.clone()) {
                    out.push(s);
                }
            }
        }
    }

    // Tier 2: same genre.
    if let Some(g) = genre.as_deref() {
        if out.len() < target {
            let more = repo::random_songs(&state.pool, target as i64 * 2, None, None, Some(g))
                .await
                .unwrap_or_default();
            for s in more {
                if out.len() >= target {
                    break;
                }
                if seen.insert(s.id.clone()) {
                    out.push(s);
                }
            }
        }
    }

    // Tier 3: random library-wide filler.
    if out.len() < target {
        let more = repo::random_songs(&state.pool, target as i64 * 2, None, None, None)
            .await
            .unwrap_or_default();
        for s in more {
            if out.len() >= target {
                break;
            }
            if seen.insert(s.id.clone()) {
                out.push(s);
            }
        }
    }

    out
}

async fn songs_to_items_result(state: &JellyfinState, songs: Vec<Song>) -> HttpResponse {
    let mut dtos = Vec::with_capacity(songs.len());
    for s in &songs {
        dtos.push(song_to_dto(state, s).await);
    }
    let total = dtos.len() as i32;
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    })
}

fn instant_mix_limit(q: &InstantMixQuery) -> i64 {
    q.limit.unwrap_or(50).max(1)
}

/// `GET /Albums/{itemId}/InstantMix`
pub async fn album_instant_mix(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<InstantMixQuery>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "album" {
        return HttpResponse::NotFound().finish();
    }
    let Ok(Some(al)) = repo::find_album(&state.pool, &native).await else {
        return HttpResponse::NotFound().finish();
    };
    // Genre for an album isn't stored on the row — sample one song from the
    // album to get a representative genre. Cheap: it's already in `songs`.
    let genre = repo::songs_by_album(&state.pool, &native)
        .await
        .unwrap_or_default()
        .into_iter()
        .find_map(|s| s.genre);
    let songs = build_instant_mix(
        &state,
        instant_mix_limit(&query),
        Vec::new(),
        Some(al.artist_id.clone()),
        genre,
    )
    .await;
    songs_to_items_result(&state, songs).await
}

/// `GET /Artists/{itemId}/InstantMix`
pub async fn artist_instant_mix(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<InstantMixQuery>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "artist" {
        return HttpResponse::NotFound().finish();
    }
    let songs = build_instant_mix(
        &state,
        instant_mix_limit(&query),
        Vec::new(),
        Some(native),
        None,
    )
    .await;
    songs_to_items_result(&state, songs).await
}

/// `GET /Songs/{itemId}/InstantMix`
pub async fn song_instant_mix(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<InstantMixQuery>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "song" {
        return HttpResponse::NotFound().finish();
    }
    let Ok(Some(seed)) = repo::find_song(&state.pool, &native).await else {
        return HttpResponse::NotFound().finish();
    };
    let artist_id = seed.artist_id.clone();
    let genre = seed.genre.clone();
    let songs = build_instant_mix(
        &state,
        instant_mix_limit(&query),
        vec![seed],
        Some(artist_id),
        genre,
    )
    .await;
    songs_to_items_result(&state, songs).await
}

/// `GET /Playlists/{itemId}/InstantMix` — take the playlist's songs in
/// order, then top up with same-genre + random-library filler.
pub async fn playlist_instant_mix(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<InstantMixQuery>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "playlist" {
        return HttpResponse::NotFound().finish();
    }
    let limit = instant_mix_limit(&query);
    let playlist_songs = repo::playlist_songs(&state.pool, &native)
        .await
        .unwrap_or_default();
    let genre = playlist_songs.iter().find_map(|s| s.genre.clone());
    let songs = build_instant_mix(&state, limit, playlist_songs, None, genre).await;
    songs_to_items_result(&state, songs).await
}

/// `GET /MusicGenres/{name}/InstantMix`
pub async fn genre_instant_mix(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<InstantMixQuery>,
) -> HttpResponse {
    let name = path.into_inner();
    let songs = build_instant_mix(
        &state,
        instant_mix_limit(&query),
        Vec::new(),
        None,
        Some(name),
    )
    .await;
    songs_to_items_result(&state, songs).await
}

/// `GET /MusicGenres/InstantMix?id=…` — same as the path-parameterised
/// variant but the seed genre is identified by GUID. smolsonic doesn't emit
/// Genre BaseItems, so any provided `id` won't resolve — return an empty
/// result rather than 404 to match real-server behaviour (the client
/// gracefully hides the empty rail).
pub async fn genre_instant_mix_by_id(
    _user: AuthedUser,
    _state: web::Data<JellyfinState>,
    _query: web::Query<InstantMixQuery>,
) -> HttpResponse {
    HttpResponse::Ok().json(ItemsResult {
        items: vec![],
        total_record_count: 0,
        start_index: 0,
    })
}

/// `GET /Items/{itemId}/InstantMix` — dispatch by resolved kind.
pub async fn item_instant_mix(
    user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<InstantMixQuery>,
) -> HttpResponse {
    let raw = path.into_inner();
    let g = mapping::normalize_guid(&raw);
    let Some((kind, _native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    // Re-dispatch — each per-kind handler already resolves and validates the
    // GUID. Wrapping in a Path::from keeps the signatures uniform.
    let path = web::Path::from(raw);
    match kind.as_str() {
        "song" => song_instant_mix(user, state, path, query).await,
        "album" => album_instant_mix(user, state, path, query).await,
        "artist" => artist_instant_mix(user, state, path, query).await,
        "playlist" => playlist_instant_mix(user, state, path, query).await,
        _ => HttpResponse::NotFound().finish(),
    }
}

// ── Similar ─────────────────────────────────────────────────────────────────
//
// Spec (Jellyfin OpenAPI, Library tag): six endpoints — /Albums, /Artists,
// /Movies, /Trailers, /Shows, /Items — each returning a
// `BaseItemDtoQueryResult` of items similar to the seed.
//
// Similarity is delegated to the Last.fm + MusicBrainz plugins in
// `similar::SimilarProviders`. Neither is required — when both are absent
// (no `[lastfm]` or `[musicbrainz]` block in the config), every Similar
// endpoint short-circuits to an empty result. Movie/Trailer/Shows never
// consult a provider (Last.fm and MB are music-only) and always return
// empty regardless.

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `user_id` and `fields` are accepted for spec-compliance but not consulted.
pub struct SimilarQuery {
    #[serde(default, alias = "Limit")]
    pub limit: Option<i64>,
    #[serde(default, alias = "ExcludeArtistIds")]
    pub exclude_artist_ids: Option<String>,
    #[serde(default, alias = "UserId", rename = "userId")]
    pub user_id: Option<String>,
    #[serde(default, alias = "Fields")]
    pub fields: Option<String>,
}

fn similar_limit(q: &SimilarQuery) -> usize {
    q.limit.unwrap_or(12).max(1) as usize
}

/// Parse `excludeArtistIds=<guid>,<guid>` into the set of *native* artist
/// ids we should drop from the result. Unknown GUIDs are silently ignored.
async fn resolve_exclude_artists(state: &JellyfinState, csv: Option<&str>) -> Vec<String> {
    let Some(csv) = csv else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for raw in csv.split(',') {
        let g = mapping::normalize_guid(raw.trim());
        if let Some((kind, native)) = resolve_native(state, &g).await {
            if kind == "artist" {
                out.push(native);
            }
        }
    }
    out
}

/// Resolve provider-returned artist names back to library `Artist` rows via
/// case-insensitive name match. Drops names we don't have.
async fn library_artists_by_name(
    state: &JellyfinState,
    names: &[String],
    exclude_artist_ids: &[String],
) -> Vec<crate::models::Artist> {
    if names.is_empty() {
        return Vec::new();
    }
    let all = repo::all_artists(&state.pool).await.unwrap_or_default();
    let lower_index: std::collections::HashMap<String, crate::models::Artist> = all
        .into_iter()
        .map(|a| (a.name.to_ascii_lowercase(), a))
        .collect();
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for name in names {
        if let Some(a) = lower_index.get(&name.to_ascii_lowercase()) {
            if exclude_artist_ids.contains(&a.id) {
                continue;
            }
            if seen.insert(a.id.clone()) {
                out.push(a.clone());
            }
        }
    }
    out
}

async fn empty_items_result() -> HttpResponse {
    HttpResponse::Ok().json(ItemsResult {
        items: vec![],
        total_record_count: 0,
        start_index: 0,
    })
}

pub async fn album_similar(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<SimilarQuery>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "album" {
        return HttpResponse::NotFound().finish();
    }
    let Ok(Some(seed_album)) = repo::find_album(&state.pool, &native).await else {
        return HttpResponse::NotFound().finish();
    };
    if !state.similar.any_enabled() {
        return empty_items_result().await;
    }

    let q = query.into_inner();
    let limit = similar_limit(&q);
    let exclude = resolve_exclude_artists(&state, q.exclude_artist_ids.as_deref()).await;
    let names = state
        .similar
        .similar_artist_names(&state.pool, &seed_album.artist_id, &seed_album.artist)
        .await;
    let similar_artists = library_artists_by_name(&state, &names, &exclude).await;

    // For each similar artist, collect their albums. Skip the seed album
    // itself even though it shouldn't appear (safety belt).
    let mut dtos: Vec<BaseItemDto> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(seed_album.id.clone());
    for artist in similar_artists {
        if dtos.len() >= limit {
            break;
        }
        let albums = repo::albums_by_artist(&state.pool, &artist.id)
            .await
            .unwrap_or_default();
        for al in albums {
            if dtos.len() >= limit {
                break;
            }
            if seen.insert(al.id.clone()) {
                dtos.push(album_to_dto(&state, &al).await);
            }
        }
    }
    let total = dtos.len() as i32;
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    })
}

pub async fn artist_similar(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<SimilarQuery>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "artist" {
        return HttpResponse::NotFound().finish();
    }
    let Ok(Some(seed)) = repo::find_artist(&state.pool, &native).await else {
        return HttpResponse::NotFound().finish();
    };
    if !state.similar.any_enabled() {
        return empty_items_result().await;
    }

    let q = query.into_inner();
    let limit = similar_limit(&q);
    let mut exclude = resolve_exclude_artists(&state, q.exclude_artist_ids.as_deref()).await;
    // Always exclude the seed itself.
    exclude.push(seed.id.clone());

    let names = state
        .similar
        .similar_artist_names(&state.pool, &seed.id, &seed.name)
        .await;
    let mut library = library_artists_by_name(&state, &names, &exclude).await;
    library.truncate(limit);

    let mut dtos = Vec::with_capacity(library.len());
    for a in &library {
        dtos.push(artist_to_dto(&state, a).await);
    }
    let total = dtos.len() as i32;
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    })
}

/// `GET /Movies/{id}/Similar`, `/Trailers/{id}/Similar`, `/Shows/{id}/Similar`.
/// Last.fm / MusicBrainz are music-only, so the video/TV variants have
/// nothing to consult and always return an empty result. We still validate
/// the item id — a 404 for unknown GUIDs matches spec.
pub async fn video_similar(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    _query: web::Query<SimilarQuery>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    match resolve_native(&state, &g).await {
        Some(_) => empty_items_result().await,
        None => HttpResponse::NotFound().finish(),
    }
}

/// `GET /Items/{id}/Similar` — dispatch by kind. Songs, playlists and other
/// kinds not covered above respond with an empty result.
pub async fn item_similar(
    user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<SimilarQuery>,
) -> HttpResponse {
    let raw = path.into_inner();
    let g = mapping::normalize_guid(&raw);
    let Some((kind, _native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    let path = web::Path::from(raw);
    match kind.as_str() {
        "album" => album_similar(user, state, path, query).await,
        "artist" => artist_similar(user, state, path, query).await,
        "video" => video_similar(user, state, path, query).await,
        _ => empty_items_result().await,
    }
}

// ── RemoteImage ─────────────────────────────────────────────────────────────
//
// Spec (Jellyfin OpenAPI, RemoteImage tag):
//   GET  /Items/{id}/RemoteImages?type=&providerName=&startIndex=&limit=
//        &includeAllLanguages=            → RemoteImageResult
//   POST /Items/{id}/RemoteImages/Download?type=&imageUrl=  → 204
//   GET  /Items/{id}/RemoteImages/Providers                 → [ImageProviderInfo]
//
// Sourced from the Last.fm + MusicBrainz plugins we already wired up for
// Similar. Only `Primary` (cover art) is supported — smolsonic doesn't
// model Art/Backdrop/Banner/etc. Requests for other types return an empty
// image list on the search endpoint and 400 on Download.

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `include_all_languages` is accepted per spec but not filtered on.
pub struct RemoteImagesQuery {
    #[serde(default, alias = "Type", rename = "type")]
    pub image_type: Option<String>,
    #[serde(default, alias = "StartIndex", rename = "startIndex")]
    pub start_index: Option<i64>,
    #[serde(default, alias = "Limit")]
    pub limit: Option<i64>,
    #[serde(default, alias = "ProviderName", rename = "providerName")]
    pub provider_name: Option<String>,
    #[serde(default, alias = "IncludeAllLanguages", rename = "includeAllLanguages")]
    pub include_all_languages: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DownloadImageQuery {
    #[serde(default, alias = "Type", rename = "type")]
    pub image_type: Option<String>,
    #[serde(default, alias = "ImageUrl", rename = "imageUrl")]
    pub image_url: Option<String>,
}

fn is_primary(kind: Option<&str>) -> bool {
    // Absent `type` → default to Primary (Findroid omits it on the initial
    // rail render). Any other explicit value → not Primary.
    match kind {
        None => true,
        Some(s) => s.eq_ignore_ascii_case("Primary"),
    }
}

/// Resolve `item_guid` to (kind, native_id). For songs we hop up to the
/// containing album because that's where the artwork actually lives.
async fn remote_image_target(
    state: &JellyfinState,
    item_guid: &str,
) -> Result<(String, crate::models::Album), HttpResponse> {
    let g = mapping::normalize_guid(item_guid);
    let Some((kind, native)) = resolve_native(state, &g).await else {
        return Err(HttpResponse::NotFound().finish());
    };
    match kind.as_str() {
        "album" => match repo::find_album(&state.pool, &native).await {
            Ok(Some(a)) => Ok(("album".to_string(), a)),
            _ => Err(HttpResponse::NotFound().finish()),
        },
        "song" => {
            let Ok(Some(s)) = repo::find_song(&state.pool, &native).await else {
                return Err(HttpResponse::NotFound().finish());
            };
            match repo::find_album(&state.pool, &s.album_id).await {
                Ok(Some(a)) => Ok(("song".to_string(), a)),
                _ => Err(HttpResponse::NotFound().finish()),
            }
        }
        "artist" => Err(HttpResponse::NotFound().finish()),
        _ => Err(HttpResponse::NotFound().finish()),
    }
}

fn enabled_provider_names(state: &JellyfinState) -> Vec<String> {
    let mut out = Vec::new();
    if state.similar.lastfm.is_some() {
        out.push("Last.fm".to_string());
    }
    if state.similar.musicbrainz.is_some() {
        out.push("MusicBrainz".to_string());
    }
    out
}

pub async fn remote_images(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<RemoteImagesQuery>,
) -> HttpResponse {
    let (_kind, album) = match remote_image_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let q = query.into_inner();
    let providers_available = enabled_provider_names(&state);

    // Non-Primary type → the query is well-formed but we have nothing.
    if !is_primary(q.image_type.as_deref()) {
        return HttpResponse::Ok().json(super::dto::RemoteImageResult {
            images: vec![],
            total_record_count: 0,
            providers: providers_available,
        });
    }

    let mut images: Vec<super::dto::RemoteImageInfo> = Vec::new();
    let want_lastfm = matches_provider(&q.provider_name, "Last.fm");
    let want_mb = matches_provider(&q.provider_name, "MusicBrainz");

    if want_lastfm {
        if let Some(lf) = &state.similar.lastfm {
            match lf
                .album_image_urls_cached(&state.pool, &album.id, &album.artist, &album.title)
                .await
            {
                Ok(urls) => {
                    for url in urls {
                        images.push(super::dto::RemoteImageInfo {
                            provider_name: Some("Last.fm".to_string()),
                            url: Some(url),
                            thumbnail_url: None,
                            height: None,
                            width: None,
                            community_rating: None,
                            vote_count: None,
                            language: Some("en".to_string()),
                            image_type: "Primary",
                        });
                    }
                }
                Err(e) => tracing::warn!("lastfm album.getInfo({}): {e}", album.title),
            }
        }
    }
    if want_mb {
        if let Some(mb) = &state.similar.musicbrainz {
            match mb
                .album_image_urls_cached(&state.pool, &album.id, &album.artist, &album.title)
                .await
            {
                Ok(urls) => {
                    for url in urls {
                        images.push(super::dto::RemoteImageInfo {
                            provider_name: Some("MusicBrainz".to_string()),
                            url: Some(url),
                            thumbnail_url: None,
                            height: None,
                            width: None,
                            community_rating: None,
                            vote_count: None,
                            language: None,
                            image_type: "Primary",
                        });
                    }
                }
                Err(e) => tracing::warn!("mb coverart({}): {e}", album.title),
            }
        }
    }

    // Honour StartIndex / Limit.
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let take = q.limit.map(|n| n.max(1) as usize).unwrap_or(usize::MAX);
    let total = images.len() as i32;
    let sliced: Vec<super::dto::RemoteImageInfo> =
        images.into_iter().skip(start).take(take).collect();
    HttpResponse::Ok().json(super::dto::RemoteImageResult {
        images: sliced,
        total_record_count: total,
        providers: providers_available,
    })
}

fn matches_provider(requested: &Option<String>, provider: &str) -> bool {
    match requested {
        None => true,
        Some(s) => s.eq_ignore_ascii_case(provider),
    }
}

pub async fn remote_image_providers(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    // Still 404 on unknown items — real Jellyfin behaves the same.
    let g = mapping::normalize_guid(&path.into_inner());
    if resolve_native(&state, &g).await.is_none() {
        return HttpResponse::NotFound().finish();
    }
    let list: Vec<super::dto::ImageProviderInfo> = enabled_provider_names(&state)
        .into_iter()
        .map(|name| super::dto::ImageProviderInfo {
            name,
            supported_images: vec!["Primary"],
        })
        .collect();
    HttpResponse::Ok().json(list)
}

pub async fn download_remote_image(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<DownloadImageQuery>,
) -> HttpResponse {
    let (_kind, album) = match remote_image_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let q = query.into_inner();
    if !is_primary(q.image_type.as_deref()) {
        return HttpResponse::BadRequest().body("only Primary image type is supported");
    }
    let Some(image_url) = q.image_url.filter(|u| u.starts_with("http")) else {
        return HttpResponse::BadRequest().body("imageUrl query parameter is required");
    };
    if state.similar.lastfm.is_none() && state.similar.musicbrainz.is_none() {
        return HttpResponse::BadRequest()
            .body("no image plugin enabled; configure [lastfm] or [musicbrainz]");
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build();
    let http = match http {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("build http client: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let resp = match http.get(&image_url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("download {image_url}: {e}");
            return HttpResponse::BadGateway().finish();
        }
    };
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("read image bytes: {e}");
            return HttpResponse::BadGateway().finish();
        }
    };
    let ext = ext_for_content_type(&content_type);
    let filename = format!("remote-{}.{ext}", album.id);
    let dst = state.covers_dir.join(&filename);
    if let Err(e) = std::fs::create_dir_all(&state.covers_dir) {
        tracing::error!("create covers_dir {}: {e}", state.covers_dir.display());
        return HttpResponse::InternalServerError().finish();
    }
    if let Err(e) = std::fs::write(&dst, &bytes) {
        tracing::error!("write cover {}: {e}", dst.display());
        return HttpResponse::InternalServerError().finish();
    }
    if let Err(e) = repo::set_album_cover_art(&state.pool, &album.id, &filename).await {
        tracing::error!("set_album_cover_art({}): {e}", album.id);
        return HttpResponse::InternalServerError().finish();
    }
    HttpResponse::NoContent().finish()
}

/// Naive content-type → extension. Falls back to `jpg` for anything unknown
/// because Cover Art Archive returns JPEG for `/front-*` by default.
fn ext_for_content_type(ct: &str) -> &'static str {
    let ct_lower = ct.to_ascii_lowercase();
    if ct_lower.starts_with("image/png") {
        "png"
    } else if ct_lower.starts_with("image/webp") {
        "webp"
    } else if ct_lower.starts_with("image/gif") {
        "gif"
    } else if ct_lower.starts_with("image/svg") {
        "svg"
    } else {
        "jpg"
    }
}

// ── Filter + Genre ──────────────────────────────────────────────────────────
//
// Spec (Jellyfin OpenAPI, Filter + Genre + MusicGenre tags):
//   GET /Items/Filters                → QueryFiltersLegacy
//   GET /Items/Filters2               → QueryFilters
//   GET /Genres                       → BaseItemDtoQueryResult
//   GET /Genres/{genreName}           → BaseItemDto
//   GET /MusicGenres                  → BaseItemDtoQueryResult (same shape as /Genres)
//   GET /MusicGenres/{genreName}      → BaseItemDto
//
// smolsonic derives genres from `songs.genre` (no dedicated table). Filters
// share that source; Tags / OfficialRatings / AudioLanguages /
// SubtitleLanguages have no backing data and always come out empty.

pub async fn items_counts(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    let song_count = repo::count_songs(&state.pool).await.unwrap_or(0) as i32;
    let album_count = repo::count_albums(&state.pool).await.unwrap_or(0) as i32;
    let artist_count = repo::count_artists(&state.pool).await.unwrap_or(0) as i32;
    let movie_count = repo::count_videos(&state.pool).await.unwrap_or(0) as i32;
    HttpResponse::Ok().json(super::dto::ItemCounts {
        song_count,
        album_count,
        artist_count,
        movie_count,
        ..Default::default()
    })
}

pub async fn items_filters(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    let genres: Vec<String> = repo::distinct_genres(&state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    let years = repo::distinct_years(&state.pool).await.unwrap_or_default();
    HttpResponse::Ok().json(super::dto::QueryFiltersLegacy {
        genres,
        tags: vec![],
        official_ratings: vec![],
        years,
    })
}

pub async fn items_filters2(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    let mut genres: Vec<super::dto::NameGuidPair> = Vec::new();
    for (name, _, _) in repo::distinct_genres(&state.pool).await.unwrap_or_default() {
        let id = mapping::remember_genre(&state.pool, &name)
            .await
            .unwrap_or_else(|_| mapping::genre_guid(&name));
        genres.push(super::dto::NameGuidPair {
            name: Some(name),
            id,
        });
    }
    HttpResponse::Ok().json(super::dto::QueryFilters {
        genres,
        tags: vec![],
        audio_languages: vec![],
        subtitle_languages: vec![],
    })
}

async fn genre_to_dto(state: &JellyfinState, name: &str) -> Option<BaseItemDto> {
    let (real_name, song_count, album_count) = repo::find_genre_stats(&state.pool, name)
        .await
        .ok()
        .flatten()?;
    let id = mapping::remember_genre(&state.pool, &real_name)
        .await
        .unwrap_or_else(|_| mapping::genre_guid(&real_name));
    Some(BaseItemDto {
        id,
        server_id: Some(state.server_id.clone()),
        name: Some(real_name.clone()),
        item_type: "MusicGenre",
        media_type: "Unknown",
        is_folder: Some(true),
        sort_name: Some(real_name),
        location_type: Some("FileSystem"),
        song_count: Some(song_count as i32),
        album_count: Some(album_count as i32),
        child_count: Some(song_count as i32),
        ..Default::default()
    })
}

pub async fn genres_list(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let q = parse_items_query(&req);
    let rows = repo::distinct_genres(&state.pool).await.unwrap_or_default();
    // Apply NameStartsWith / SearchTerm from the query.
    let filtered: Vec<(String, i64, i64)> = rows
        .into_iter()
        .filter(|(n, _, _)| {
            if let Some(prefix) = q.name_starts_with.as_deref() {
                if !n
                    .to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
                {
                    return false;
                }
            }
            if let Some(term) = q.search_term.as_deref() {
                if !n.to_ascii_lowercase().contains(&term.to_ascii_lowercase()) {
                    return false;
                }
            }
            true
        })
        .collect();
    let total = filtered.len() as i32;
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let take = q.limit.map(|n| n.max(1) as usize).unwrap_or(usize::MAX);
    let mut dtos: Vec<BaseItemDto> = Vec::new();
    for (name, song_count, album_count) in filtered.into_iter().skip(start).take(take) {
        let id = mapping::remember_genre(&state.pool, &name)
            .await
            .unwrap_or_else(|_| mapping::genre_guid(&name));
        dtos.push(BaseItemDto {
            id,
            server_id: Some(state.server_id.clone()),
            name: Some(name.clone()),
            item_type: "MusicGenre",
            media_type: "Unknown",
            is_folder: Some(true),
            sort_name: Some(name),
            location_type: Some("FileSystem"),
            song_count: Some(song_count as i32),
            album_count: Some(album_count as i32),
            child_count: Some(song_count as i32),
            ..Default::default()
        });
    }
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: start as i32,
    })
}

pub async fn genre_by_name(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let name = path.into_inner();
    match genre_to_dto(&state, &name).await {
        Some(dto) => HttpResponse::Ok().json(dto),
        None => HttpResponse::NotFound().finish(),
    }
}

async fn year_to_dto(state: &JellyfinState, year: i32) -> Option<BaseItemDto> {
    let (song_count, album_count) = repo::find_year_stats(&state.pool, year)
        .await
        .ok()
        .flatten()?;
    let id = mapping::remember_year(&state.pool, year)
        .await
        .unwrap_or_else(|_| mapping::year_guid(year));
    let name = year.to_string();
    Some(BaseItemDto {
        id,
        server_id: Some(state.server_id.clone()),
        name: Some(name.clone()),
        item_type: "Year",
        media_type: "Unknown",
        is_folder: Some(true),
        sort_name: Some(name),
        location_type: Some("FileSystem"),
        production_year: Some(year),
        song_count: Some(song_count as i32),
        album_count: Some(album_count as i32),
        child_count: Some((song_count + album_count) as i32),
        ..Default::default()
    })
}

pub async fn years_list(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let q = parse_items_query(&req);
    let years = repo::distinct_years(&state.pool).await.unwrap_or_default();
    let total = years.len() as i32;
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let take = q.limit.map(|n| n.max(1) as usize).unwrap_or(usize::MAX);
    // sortOrder=Descending is common on the year rail — flip when asked.
    let ordered: Vec<i32> = if q
        .sort_order
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("Descending"))
        .unwrap_or(false)
    {
        years.into_iter().rev().collect()
    } else {
        years
    };
    let mut dtos: Vec<BaseItemDto> = Vec::new();
    for year in ordered.into_iter().skip(start).take(take) {
        if let Some(dto) = year_to_dto(&state, year).await {
            dtos.push(dto);
        }
    }
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: start as i32,
    })
}

pub async fn year_by_value(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<i32>,
) -> HttpResponse {
    let year = path.into_inner();
    match year_to_dto(&state, year).await {
        Some(dto) => HttpResponse::Ok().json(dto),
        None => HttpResponse::NotFound().finish(),
    }
}

/// `/Persons` and `/Studios` — smolsonic doesn't scan Persons (composers,
/// performers) or Studios (labels) into the DB. Return an empty result so
/// clients render "no persons/studios" cleanly rather than 404-ing.
pub async fn persons_list(_user: AuthedUser) -> HttpResponse {
    HttpResponse::Ok().json(ItemsResult {
        items: vec![],
        total_record_count: 0,
        start_index: 0,
    })
}

pub async fn person_by_name(_user: AuthedUser, _path: web::Path<String>) -> HttpResponse {
    HttpResponse::NotFound().finish()
}

pub async fn studios_list(_user: AuthedUser) -> HttpResponse {
    HttpResponse::Ok().json(ItemsResult {
        items: vec![],
        total_record_count: 0,
        start_index: 0,
    })
}

pub async fn studio_by_name(_user: AuthedUser, _path: web::Path<String>) -> HttpResponse {
    HttpResponse::NotFound().finish()
}

// ── Images ───────────────────────────────────────────────────────────────────

fn cover_path_for(state: &JellyfinState, filename: &str) -> PathBuf {
    state.covers_dir.join(filename)
}

/// Resolved image source: either a filename under `covers_dir` or an absolute
/// path that points at a sibling poster file (or an ffmpeg-generated thumb).
enum ImageSource {
    InCoversDir(String),
    Absolute(PathBuf),
}

async fn resolve_image_source(state: &JellyfinState, guid: &str) -> Option<ImageSource> {
    let g = mapping::normalize_guid(guid);
    let (kind, native) = mapping::lookup(&state.pool, &g).await.ok().flatten()?;
    match kind.as_str() {
        "album" => repo::find_album(&state.pool, &native)
            .await
            .ok()
            .flatten()
            .and_then(|a| a.cover_art)
            .map(ImageSource::InCoversDir),
        "song" => {
            let song = repo::find_song(&state.pool, &native).await.ok().flatten()?;
            let filename = if song.cover_art.is_some() {
                song.cover_art
            } else {
                repo::find_album(&state.pool, &song.album_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|a| a.cover_art)
            }?;
            Some(ImageSource::InCoversDir(filename))
        }
        "artist" => repo::albums_by_artist(&state.pool, &native)
            .await
            .ok()
            .unwrap_or_default()
            .into_iter()
            .find_map(|a| a.cover_art)
            .map(ImageSource::InCoversDir),
        "video" => repo::find_video(&state.pool, &native)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.poster_path)
            .map(|p| ImageSource::Absolute(PathBuf::from(p))),
        _ => None,
    }
}

pub async fn item_image(
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    let (item_guid, _kind) = path.into_inner();
    serve_cover(&state, &item_guid).await
}

pub async fn item_image_by_index(
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String, i32)>,
) -> HttpResponse {
    let (item_guid, _kind, _idx) = path.into_inner();
    serve_cover(&state, &item_guid).await
}

async fn serve_cover(state: &JellyfinState, item_guid: &str) -> HttpResponse {
    let Some(source) = resolve_image_source(state, item_guid).await else {
        return HttpResponse::NotFound().finish();
    };
    let full = match source {
        ImageSource::InCoversDir(filename) => cover_path_for(state, &filename),
        ImageSource::Absolute(p) => p,
    };
    match std::fs::read(&full) {
        Ok(data) => {
            let mime = mime_guess::from_path(&full)
                .first_or_octet_stream()
                .to_string();
            HttpResponse::Ok().content_type(mime).body(data)
        }
        Err(e) => {
            tracing::warn!("jellyfin: image read {}: {e}", full.display());
            HttpResponse::NotFound().finish()
        }
    }
}

// ── PlaybackInfo ─────────────────────────────────────────────────────────────

pub async fn playback_info(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    let media_sources = match kind.as_str() {
        "song" => {
            let Ok(Some(song)) = repo::find_song(&state.pool, &native).await else {
                return HttpResponse::NotFound().finish();
            };
            song_to_dto(&state, &song).await.media_sources
        }
        "video" => {
            let Ok(Some(video)) = repo::find_video(&state.pool, &native).await else {
                return HttpResponse::NotFound().finish();
            };
            video_to_dto(&state, &video).await.media_sources
        }
        _ => return HttpResponse::BadRequest().finish(),
    };
    HttpResponse::Ok().json(PlaybackInfoResponse {
        media_sources: media_sources.unwrap_or_default(),
        play_session_id: Some(auth::random_hex(8)),
    })
}

// ── Audio stream ─────────────────────────────────────────────────────────────

async fn stream_by_guid(state: &JellyfinState, guid: &str, req: &HttpRequest) -> HttpResponse {
    // Streaming endpoints accept api_key as a query param, so we authorize
    // here instead of through the FromRequest extractor.
    let token = auth::extract_token(req);
    let authorized = match token {
        Some(t) => auth::token_valid(&state.pool, &t).await,
        None => false,
    };
    if !authorized {
        return HttpResponse::Unauthorized().finish();
    }

    let g = mapping::normalize_guid(guid);
    let Some((kind, native)) = resolve_native(state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "song" {
        return HttpResponse::BadRequest().finish();
    }
    let Ok(Some(song)) = repo::find_song(&state.pool, &native).await else {
        return HttpResponse::NotFound().finish();
    };
    serve_song(&song, req)
}

pub async fn audio_stream(
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    stream_by_guid(&state, &path.into_inner(), &req).await
}

pub async fn audio_stream_ext(
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
    req: HttpRequest,
) -> HttpResponse {
    let (id, _ext) = path.into_inner();
    stream_by_guid(&state, &id, &req).await
}

pub async fn audio_universal(
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    stream_by_guid(&state, &path.into_inner(), &req).await
}

fn serve_song(song: &Song, req: &HttpRequest) -> HttpResponse {
    serve_file(&song.path, &song.content_type, req)
}

fn video_content_type(container: &str) -> &'static str {
    match container {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

fn serve_file(path_str: &str, content_type: &str, req: &HttpRequest) -> HttpResponse {
    let path = PathBuf::from(path_str);
    let file_size = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::error!("jellyfin stream stat {path_str}: {e}");
            return HttpResponse::InternalServerError().finish();
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
                                .content_type(content_type.to_string())
                                .insert_header(("Accept-Ranges", "bytes"))
                                .insert_header(("Content-Length", n.to_string()))
                                .insert_header((
                                    "Content-Range",
                                    format!("bytes {}-{}/{}", start, actual_end, file_size),
                                ))
                                .body(buf)
                        }
                        Err(e) => {
                            tracing::error!("jellyfin stream open {path_str}: {e}");
                            HttpResponse::InternalServerError().finish()
                        }
                    };
                }
            }
        }
    }

    match std::fs::read(&path) {
        Ok(data) => HttpResponse::Ok()
            .content_type(content_type.to_string())
            .insert_header(("Accept-Ranges", "bytes"))
            .insert_header(("Content-Length", file_size.to_string()))
            .body(data),
        Err(e) => {
            tracing::error!("jellyfin stream read {path_str}: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

// ── Lyric ────────────────────────────────────────────────────────────────────
//
// Spec (Jellyfin OpenAPI, Lyric tag):
//   GET    /Audio/{itemId}/Lyrics                             → LyricDto
//   POST   /Audio/{itemId}/Lyrics?fileName=…   (body text/…)  → LyricDto
//   DELETE /Audio/{itemId}/Lyrics                             → 204
//   GET    /Audio/{itemId}/RemoteSearch/Lyrics                → []
//   POST   /Audio/{itemId}/RemoteSearch/Lyrics/{lyricId}      → 404
//   GET    /Providers/Lyrics/{lyricId}                        → 404
//
// smolsonic reads/writes a `.lrc` sidecar next to the audio file. The three
// remote endpoints are accepted for spec-compliance but always return
// empty / 404 — we have no remote lyric provider.

async fn audio_native_song(
    state: &JellyfinState,
    guid: &str,
) -> Result<crate::models::Song, HttpResponse> {
    let g = mapping::normalize_guid(guid);
    let Some((kind, native)) = resolve_native(state, &g).await else {
        return Err(HttpResponse::NotFound().finish());
    };
    if kind != "song" {
        return Err(HttpResponse::NotFound().finish());
    }
    match repo::find_song(&state.pool, &native).await {
        Ok(Some(s)) => Ok(s),
        _ => Err(HttpResponse::NotFound().finish()),
    }
}

pub async fn get_lyrics(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let song = match audio_native_song(&state, &path.into_inner()).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let audio_path = std::path::Path::new(&song.path);
    let Some(sidecar) = super::lyrics::find_sidecar(audio_path) else {
        return HttpResponse::NotFound().finish();
    };
    let Ok(source) = std::fs::read_to_string(&sidecar) else {
        tracing::warn!(
            "jellyfin: failed to read lyric sidecar {}",
            sidecar.display()
        );
        return HttpResponse::NotFound().finish();
    };
    HttpResponse::Ok().json(super::lyrics::parse_lrc(&source))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `file_name` is required by the spec but we key on the audio file's stem.
pub struct UploadLyricsQuery {
    #[serde(default, alias = "FileName", rename = "fileName")]
    pub file_name: Option<String>,
}

pub async fn upload_lyrics(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    _query: web::Query<UploadLyricsQuery>,
    body: web::Bytes,
) -> HttpResponse {
    let song = match audio_native_song(&state, &path.into_inner()).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let sidecar = super::lyrics::sidecar_path(std::path::Path::new(&song.path));
    if let Err(e) = std::fs::write(&sidecar, &body) {
        tracing::error!("jellyfin: write lyric sidecar {}: {e}", sidecar.display());
        return HttpResponse::InternalServerError().finish();
    }
    let source = String::from_utf8_lossy(&body).into_owned();
    HttpResponse::Ok().json(super::lyrics::parse_lrc(&source))
}

pub async fn delete_lyrics(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let song = match audio_native_song(&state, &path.into_inner()).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let audio_path = std::path::Path::new(&song.path);
    let Some(sidecar) = super::lyrics::find_sidecar(audio_path) else {
        // No sidecar → treat DELETE as a success no-op. Real Jellyfin
        // returns 204 in this case as well.
        return HttpResponse::NoContent().finish();
    };
    if let Err(e) = std::fs::remove_file(&sidecar) {
        tracing::error!("jellyfin: delete lyric sidecar {}: {e}", sidecar.display());
        return HttpResponse::InternalServerError().finish();
    }
    HttpResponse::NoContent().finish()
}

/// `GET /Audio/{itemId}/RemoteSearch/Lyrics` — smolsonic has no remote
/// provider; return an empty array so the client's "search online" button
/// stops spinning cleanly instead of erroring.
pub async fn remote_search_lyrics(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    // Still validate the itemId — a 404 for an unknown GUID matches how
    // real Jellyfin behaves before it hits the provider layer.
    match audio_native_song(&state, &path.into_inner()).await {
        Ok(_) => HttpResponse::Ok().json(Vec::<Value>::new()),
        Err(resp) => resp,
    }
}

pub async fn download_remote_lyric(
    _user: AuthedUser,
    _state: web::Data<JellyfinState>,
    _path: web::Path<(String, String)>,
) -> HttpResponse {
    HttpResponse::NotFound().finish()
}

pub async fn get_remote_lyric(
    _user: AuthedUser,
    _state: web::Data<JellyfinState>,
    _path: web::Path<String>,
) -> HttpResponse {
    HttpResponse::NotFound().finish()
}

// ── Video stream ─────────────────────────────────────────────────────────────

async fn video_by_guid(state: &JellyfinState, guid: &str, req: &HttpRequest) -> HttpResponse {
    let token = auth::extract_token(req);
    let authorized = match token {
        Some(t) => auth::token_valid(&state.pool, &t).await,
        None => false,
    };
    if !authorized {
        return HttpResponse::Unauthorized().finish();
    }

    let g = mapping::normalize_guid(guid);
    let Some((kind, native)) = resolve_native(state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    if kind != "video" {
        return HttpResponse::BadRequest().finish();
    }
    let Ok(Some(video)) = repo::find_video(&state.pool, &native).await else {
        return HttpResponse::NotFound().finish();
    };
    serve_file(&video.path, video_content_type(&video.container), req)
}

pub async fn video_stream(
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    video_by_guid(&state, &path.into_inner(), &req).await
}

/// `GET /Items/{id}/File` — Finamp / just_audio stream the original file via
/// this endpoint, passing the token as `?ApiKey=`. Dispatches to the song or
/// video file streamer based on what kind of item the GUID points to.
pub async fn item_file_stream(
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    let token = auth::extract_token(&req);
    let authorized = match token {
        Some(t) => auth::token_valid(&state.pool, &t).await,
        None => false,
    };
    if !authorized {
        return HttpResponse::Unauthorized().finish();
    }

    let g = mapping::normalize_guid(&path.into_inner());
    let Some((kind, native)) = resolve_native(&state, &g).await else {
        return HttpResponse::NotFound().finish();
    };
    match kind.as_str() {
        "song" => match repo::find_song(&state.pool, &native).await {
            Ok(Some(s)) => serve_song(&s, &req),
            _ => HttpResponse::NotFound().finish(),
        },
        "video" => match repo::find_video(&state.pool, &native).await {
            Ok(Some(v)) => serve_file(&v.path, video_content_type(&v.container), &req),
            _ => HttpResponse::NotFound().finish(),
        },
        _ => HttpResponse::NotFound().finish(),
    }
}

pub async fn video_stream_ext(
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
    req: HttpRequest,
) -> HttpResponse {
    let (id, _ext) = path.into_inner();
    video_by_guid(&state, &id, &req).await
}

// ── Scrobble / sessions ──────────────────────────────────────────────────────

pub async fn sessions_capabilities(_user: AuthedUser) -> HttpResponse {
    HttpResponse::NoContent().finish()
}

/// `GET /Sessions?activeWithinSeconds=N` — Streamyfin polls this every few
/// seconds. We don't track active sessions; an empty list keeps the client
/// polling cleanly instead of 404-ing.
pub async fn sessions_list(_user: AuthedUser) -> HttpResponse {
    HttpResponse::Ok().json(Vec::<Value>::new())
}

pub async fn sessions_playing(_user: AuthedUser, _body: web::Json<Value>) -> HttpResponse {
    HttpResponse::NoContent().finish()
}

pub async fn sessions_playing_progress(_user: AuthedUser, _body: web::Json<Value>) -> HttpResponse {
    HttpResponse::NoContent().finish()
}

pub async fn sessions_playing_stopped(_user: AuthedUser, _body: web::Json<Value>) -> HttpResponse {
    HttpResponse::NoContent().finish()
}

// ── Favorites ───────────────────────────────────────────────────────────────
//
// Spec (Jellyfin OpenAPI, UserLibrary tag):
//   POST   /UserFavoriteItems/{itemId}?userId=…   → UserItemDataDto
//   DELETE /UserFavoriteItems/{itemId}?userId=…   → UserItemDataDto
//
// Older clients still hit the legacy per-user variant:
//   POST   /Users/{userId}/FavoriteItems/{itemId} → UserItemDataDto
//   DELETE /Users/{userId}/FavoriteItems/{itemId} → UserItemDataDto
//
// Both mutate a single row in the `starred` sidecar. smolsonic is
// single-user, so the `userId` param is accepted but not consulted — the
// favorite is scoped to the sole account.

/// Resolve the Jellyfin GUID at the end of the path back to a native id and
/// its kind. Returns 404 if the GUID isn't in `jf_guids`.
async fn favorite_native(
    state: &JellyfinState,
    item_guid: &str,
) -> Result<(String, String), HttpResponse> {
    let g = mapping::normalize_guid(item_guid);
    match resolve_native(state, &g).await {
        Some(pair) => Ok(pair),
        None => Err(HttpResponse::NotFound().finish()),
    }
}

async fn set_favorite(
    state: web::Data<JellyfinState>,
    item_guid: String,
    starred: bool,
) -> HttpResponse {
    let (_kind, native) = match favorite_native(&state, &item_guid).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let now = now_iso();
    let result = if starred {
        repo::star(&state.pool, &native, &now).await
    } else {
        repo::unstar(&state.pool, &native).await
    };
    if let Err(e) = result {
        tracing::error!("jellyfin: favorite mutation on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    let dashed = mapping::normalize_guid(&item_guid);
    HttpResponse::Ok().json(build_user_data(&state, &native, dashed).await)
}

pub async fn add_favorite_item(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    set_favorite(state, path.into_inner(), true).await
}

pub async fn remove_favorite_item(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    set_favorite(state, path.into_inner(), false).await
}

pub async fn add_user_favorite_item(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    set_favorite(state, item_id, true).await
}

pub async fn remove_user_favorite_item(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    set_favorite(state, item_id, false).await
}

// ── UserData / PlayedItems / Rating ─────────────────────────────────────────
//
// Spec (Jellyfin OpenAPI, UserLibrary tag):
//   GET    /UserItems/{itemId}/UserData          → UserItemDataDto
//   POST   /UserItems/{itemId}/UserData          → UserItemDataDto (body: UpdateUserItemDataDto)
//   POST   /UserPlayedItems/{itemId}?datePlayed= → UserItemDataDto (mark played, PlayCount++)
//   DELETE /UserPlayedItems/{itemId}             → UserItemDataDto (mark unplayed)
//   POST   /UserItems/{itemId}/Rating?likes=…    → UserItemDataDto (set Likes)
//   DELETE /UserItems/{itemId}/Rating            → UserItemDataDto (clear Likes)
//
// Legacy per-user forms hit by older clients (Symfonium, Finamp <=1.9):
//   POST   /Users/{userId}/PlayedItems/{itemId}
//   DELETE /Users/{userId}/PlayedItems/{itemId}
//   POST   /Users/{userId}/Items/{itemId}/UserData
//   GET    /Users/{userId}/Items/{itemId}/UserData
//   POST   /Users/{userId}/Items/{itemId}/Rating
//   DELETE /Users/{userId}/Items/{itemId}/Rating
//
// All state lives in the `user_item_data` sidecar keyed by native id; the
// `userId` param is accepted but not consulted (smolsonic is single-user).

/// Resolve `item_guid` to (native_id, dashed jf guid), 404 if unknown.
async fn user_data_target(
    state: &JellyfinState,
    item_guid: &str,
) -> Result<(String, String), HttpResponse> {
    let g = mapping::normalize_guid(item_guid);
    match resolve_native(state, &g).await {
        Some((_kind, native)) => Ok((native, g)),
        None => Err(HttpResponse::NotFound().finish()),
    }
}

async fn respond_user_data(state: &JellyfinState, native: &str, jf_guid: String) -> HttpResponse {
    HttpResponse::Ok().json(build_user_data(state, native, jf_guid).await)
}

/// `?likes=true|false` — query param for POST /UserItems/{id}/Rating (and
/// its legacy per-user variant).
#[derive(Debug, Deserialize)]
pub struct RatingQuery {
    #[serde(default, alias = "Likes")]
    pub likes: Option<bool>,
}

/// `?datePlayed=…` — optional query param for POST /UserPlayedItems/{id}
/// (and the legacy `/Users/{uid}/PlayedItems/{id}` variant). When present
/// it overrides `now_iso()` as the stamped `LastPlayedDate`.
#[derive(Debug, Deserialize)]
pub struct PlayedQuery {
    #[serde(default, alias = "DatePlayed", rename = "datePlayed")]
    pub date_played: Option<String>,
}

pub async fn get_user_item_data_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let (native, guid) = match user_data_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    respond_user_data(&state, &native, guid).await
}

pub async fn update_user_item_data_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    body: web::Json<super::dto::UpdateUserItemDataDto>,
) -> HttpResponse {
    let (native, guid) = match user_data_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = apply_update(&state, &native, body.into_inner()).await {
        tracing::error!("jellyfin: user_item_data update on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

/// Legacy `POST /Users/{userId}/Items/{itemId}/UserData`. Same payload as
/// the spec-form endpoint above.
pub async fn update_user_item_data_legacy(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
    body: web::Json<super::dto::UpdateUserItemDataDto>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    let (native, guid) = match user_data_target(&state, &item_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = apply_update(&state, &native, body.into_inner()).await {
        tracing::error!("jellyfin: user_item_data update on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

pub async fn get_user_item_data_legacy(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    let (native, guid) = match user_data_target(&state, &item_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    respond_user_data(&state, &native, guid).await
}

async fn apply_update(
    state: &JellyfinState,
    native: &str,
    body: super::dto::UpdateUserItemDataDto,
) -> anyhow::Result<()> {
    // Convert the nullable-everywhere request DTO into the two-level Option
    // shape repo::update_user_item_data expects: outer `Some` means "present
    // in the body", inner value = "the value to store" (including `None` for
    // nullable-clear on last_played_date/rating/likes).
    let update = repo::UserItemDataUpdate {
        played: body.played,
        play_count: body.play_count,
        playback_position_ticks: body.playback_position_ticks,
        // Body carries a single `Option<String>` — client can't distinguish
        // "leave alone" from "clear" here without a signal channel, so we
        // treat any present body as "write this value" (which may be None).
        last_played_date: Some(body.last_played_date),
        rating: Some(body.rating),
        likes: Some(body.likes),
    };
    repo::update_user_item_data(&state.pool, native, update).await?;
    // Favorite state is stored in `starred` — keep the two tables in sync so
    // the response reflects the requested toggle.
    if let Some(fav) = body.is_favorite {
        let now = now_iso();
        if fav {
            repo::star(&state.pool, native, &now).await?;
        } else {
            repo::unstar(&state.pool, native).await?;
        }
    }
    Ok(())
}

pub async fn mark_played_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<PlayedQuery>,
) -> HttpResponse {
    let (native, guid) = match user_data_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let when = query.into_inner().date_played.unwrap_or_else(now_iso);
    if let Err(e) = repo::mark_played(&state.pool, &native, &when).await {
        tracing::error!("jellyfin: mark_played on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

pub async fn mark_unplayed_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let (native, guid) = match user_data_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = repo::mark_unplayed(&state.pool, &native).await {
        tracing::error!("jellyfin: mark_unplayed on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

/// `POST /Users/{userId}/PlayedItems/{itemId}` — legacy per-user variant.
pub async fn mark_played_legacy(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
    query: web::Query<PlayedQuery>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    let (native, guid) = match user_data_target(&state, &item_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let when = query.into_inner().date_played.unwrap_or_else(now_iso);
    if let Err(e) = repo::mark_played(&state.pool, &native, &when).await {
        tracing::error!("jellyfin: mark_played on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

pub async fn mark_unplayed_legacy(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    let (native, guid) = match user_data_target(&state, &item_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = repo::mark_unplayed(&state.pool, &native).await {
        tracing::error!("jellyfin: mark_unplayed on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

pub async fn set_rating_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
    query: web::Query<RatingQuery>,
) -> HttpResponse {
    let (native, guid) = match user_data_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = repo::set_likes(&state.pool, &native, query.into_inner().likes).await {
        tracing::error!("jellyfin: set_likes on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

pub async fn clear_rating_endpoint(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<String>,
) -> HttpResponse {
    let (native, guid) = match user_data_target(&state, &path.into_inner()).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = repo::set_likes(&state.pool, &native, None).await {
        tracing::error!("jellyfin: clear_likes on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

pub async fn set_rating_legacy(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
    query: web::Query<RatingQuery>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    let (native, guid) = match user_data_target(&state, &item_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = repo::set_likes(&state.pool, &native, query.into_inner().likes).await {
        tracing::error!("jellyfin: set_likes on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

pub async fn clear_rating_legacy(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
) -> HttpResponse {
    let (_user_id, item_id) = path.into_inner();
    let (native, guid) = match user_data_target(&state, &item_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    if let Err(e) = repo::set_likes(&state.pool, &native, None).await {
        tracing::error!("jellyfin: clear_likes on {native}: {e}");
        return HttpResponse::InternalServerError().finish();
    }
    respond_user_data(&state, &native, guid).await
}

// ── Misc stubs that some clients probe ──────────────────────────────────────

pub async fn empty_array() -> HttpResponse {
    HttpResponse::Ok().json(Vec::<Value>::new())
}

/// 204 ack for endpoints we accept but have nothing to return (scheduled
/// tasks, scrobble pings, etc.).
pub async fn no_content() -> HttpResponse {
    HttpResponse::NoContent().finish()
}

/// Routed 404 — same wire result as the default service, but bypasses the
/// `log_unrouted` warning. Use for endpoints that *should* respond 404
/// (e.g. a user with no avatar, an unsupported WebSocket upgrade) so the
/// log stays focused on genuinely unhandled paths.
pub async fn not_found() -> HttpResponse {
    HttpResponse::NotFound().finish()
}

/// `GET/HEAD /System/Ping` — Jellyfin's heartbeat endpoint. Reference
/// server returns plain text "Jellyfin Server"; some clients (Moonfin,
/// official web) use this to decide if the server is reachable.
pub async fn system_ping() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body("Jellyfin Server")
}

/// `POST /ScheduledTasks/Running/{id}` and `POST /Library/Refresh` — kick off
/// a library rescan in the background. Returns 204 immediately; the scan
/// continues asynchronously. If a scan is already in progress, this is a
/// cheap no-op (the scanner's `running` flag prevents overlap).
pub async fn trigger_library_scan(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
) -> HttpResponse {
    use std::sync::atomic::Ordering;

    // Music
    if !state.music_scan_progress.running.load(Ordering::SeqCst) {
        let pool = state.pool.clone();
        let music_dir = state.music_dir.clone();
        let covers_dir = state.covers_dir.clone();
        let progress = state.music_scan_progress.clone();
        tokio::spawn(async move {
            tracing::info!("jellyfin: triggered music scan of {}", music_dir.display());
            if let Err(e) = crate::scanner::scan(pool, music_dir, covers_dir, progress).await {
                tracing::error!("jellyfin-triggered music scan failed: {e}");
            }
        });
    } else {
        tracing::debug!("jellyfin: music scan trigger ignored (already running)");
    }

    // Video (only if a video library is configured)
    if let Some(video_dir) = state.video_dir.clone() {
        if !state.video_scan_progress.running.load(Ordering::SeqCst) {
            let pool = state.pool.clone();
            let covers_dir = state.covers_dir.clone();
            let progress = state.video_scan_progress.clone();
            tokio::spawn(async move {
                tracing::info!("jellyfin: triggered video scan of {}", video_dir.display());
                if let Err(e) =
                    crate::video_scanner::scan(pool, video_dir, covers_dir, progress).await
                {
                    tracing::error!("jellyfin-triggered video scan failed: {e}");
                }
            });
        }
    }

    HttpResponse::NoContent().finish()
}

/// `/Items/Latest?userId=...&parentId=...&limit=N` — Findroid uses this for
/// the "Latest" rail on each library page. Returns up to `limit` items
/// inside the given parent (movies for the Movies library, albums for
/// Music). Response is a plain JSON array, not an `ItemsResult`.
pub async fn items_latest(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    items_latest_impl(state, parse_items_query(&req)).await
}

async fn items_latest_impl(state: web::Data<JellyfinState>, q: ItemsQuery) -> HttpResponse {
    let limit = q.limit.unwrap_or(16).max(1);

    if let Some(parent) = q.parent_id.as_ref() {
        let g = mapping::normalize_guid(parent);
        if g == mapping::movies_library_guid() {
            let videos = repo::all_videos(&state.pool, limit, 0)
                .await
                .unwrap_or_default();
            let mut dtos = Vec::with_capacity(videos.len());
            for v in &videos {
                dtos.push(video_to_dto(&state, v).await);
            }
            return HttpResponse::Ok().json(dtos);
        }
        if g == mapping::library_guid() {
            let albums = repo::albums_paginated(&state.pool, "newest", limit, 0)
                .await
                .unwrap_or_default();
            let mut dtos = Vec::with_capacity(albums.len());
            for a in &albums {
                dtos.push(album_to_dto(&state, a).await);
            }
            return HttpResponse::Ok().json(dtos);
        }
        if g == mapping::playlists_library_guid() {
            let playlists = repo::all_playlists(&state.pool).await.unwrap_or_default();
            let take = (limit as usize).min(playlists.len());
            let mut dtos = Vec::with_capacity(take);
            for p in playlists.iter().take(take) {
                dtos.push(playlist_to_dto(&state, p).await);
            }
            return HttpResponse::Ok().json(dtos);
        }
    }
    if wants_videos(&q) {
        let videos = repo::all_videos(&state.pool, limit, 0)
            .await
            .unwrap_or_default();
        let mut dtos = Vec::with_capacity(videos.len());
        for v in &videos {
            dtos.push(video_to_dto(&state, v).await);
        }
        return HttpResponse::Ok().json(dtos);
    }
    HttpResponse::Ok().json(Vec::<Value>::new())
}

pub async fn empty_items() -> HttpResponse {
    HttpResponse::Ok().json(ItemsResult {
        items: vec![],
        total_record_count: 0,
        start_index: 0,
    })
}

/// `/Users/{uid}/Views/{view}/Latest` — legacy per-library latest rail.
/// The view id lives in the path here (not the query), so we override
/// `parent_id` on the parsed query and delegate to `items_latest`'s logic.
pub async fn view_latest(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    path: web::Path<(String, String)>,
    req: HttpRequest,
) -> HttpResponse {
    let (_uid, view) = path.into_inner();
    let mut q = parse_items_query(&req);
    q.parent_id = Some(view);
    items_latest_impl(state, q).await
}

/// `/Items/Resume`, `/UserItems/Resume`, `/Users/{uid}/Items/Resume` —
/// items with a non-zero playback position, ordered by most-recently
/// played. Returned as a `BaseItemDtoQueryResult` per spec (not a bare
/// array like Latest).
pub async fn items_resume(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let q = parse_items_query(&req);
    let limit = q.limit.unwrap_or(12).max(1);

    // Movies-library parent → resume videos; music parent (or no parent) →
    // resume songs. `?mediaTypes=Video` also asks for videos.
    let want_videos = q
        .parent_id
        .as_deref()
        .map(mapping::normalize_guid)
        .map(|g| g == mapping::movies_library_guid())
        .unwrap_or(false)
        || includes(&q.media_types, "Video");

    if want_videos {
        let videos = repo::resume_videos(&state.pool, limit)
            .await
            .unwrap_or_default();
        let mut dtos = Vec::with_capacity(videos.len());
        for v in &videos {
            dtos.push(video_to_dto(&state, v).await);
        }
        let total = dtos.len() as i32;
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: 0,
        });
    }

    let songs = repo::resume_songs(&state.pool, limit)
        .await
        .unwrap_or_default();
    let mut dtos = Vec::with_capacity(songs.len());
    for s in &songs {
        dtos.push(song_to_dto(&state, s).await);
    }
    let total = dtos.len() as i32;
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    })
}

/// `/Items/Suggestions`, `/Users/{uid}/Items/Suggestions` — spec-defined
/// "you might like" rail. Returns random albums by default (or songs when
/// `?type=Audio` / `?mediaType=Audio`, or movies when `?type=Movie`).
pub async fn items_suggestions(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
    let q = parse_items_query(&req);
    let limit = q.limit.unwrap_or(12).max(1);

    if includes(&q.include_item_types, "Audio") || includes(&q.media_types, "Audio") {
        let songs = repo::random_songs(&state.pool, limit, None, None, None)
            .await
            .unwrap_or_default();
        let mut dtos = Vec::with_capacity(songs.len());
        for s in &songs {
            dtos.push(song_to_dto(&state, s).await);
        }
        let total = dtos.len() as i32;
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: 0,
        });
    }
    if wants_videos(&q) {
        // Use the same "random" approximation as MusicAlbum below — all_videos
        // is ordered by title but that's better than nothing.
        let videos = repo::all_videos(&state.pool, limit, 0)
            .await
            .unwrap_or_default();
        let mut dtos = Vec::with_capacity(videos.len());
        for v in &videos {
            dtos.push(video_to_dto(&state, v).await);
        }
        let total = dtos.len() as i32;
        return HttpResponse::Ok().json(ItemsResult {
            items: dtos,
            total_record_count: total,
            start_index: 0,
        });
    }

    // Default → random albums. Reuse `albums_paginated("random", …)` which
    // ORDER BY RANDOM()s the table.
    let albums = repo::albums_paginated(&state.pool, "random", limit, 0)
        .await
        .unwrap_or_default();
    let mut dtos = Vec::with_capacity(albums.len());
    for a in &albums {
        dtos.push(album_to_dto(&state, a).await);
    }
    let total = dtos.len() as i32;
    HttpResponse::Ok().json(ItemsResult {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    })
}

/// `/UserViews?userId=...&includeHidden=...` — Findroid uses this instead of
/// the path-parametric `/Users/{id}/Views`. Same payload.
pub async fn user_views_query(_user: AuthedUser, state: web::Data<JellyfinState>) -> HttpResponse {
    let views = all_library_views(&state);
    let total = views.len() as i32;
    HttpResponse::Ok().json(ViewsResult {
        items: views,
        total_record_count: total,
        start_index: 0,
    })
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchHintsQuery {
    pub search_term: Option<String>,
    pub limit: Option<i64>,
    pub start_index: Option<i64>,
    pub include_item_types: Option<String>,
    pub user_id: Option<String>,
}

impl Default for SearchHintsQuery {
    fn default() -> Self {
        Self {
            search_term: None,
            limit: None,
            start_index: None,
            include_item_types: None,
            user_id: None,
        }
    }
}

/// `/Search/Hints?searchTerm=...` — backed by the existing FTS index. Returns
/// up to `limit` artist/album/song matches.
pub async fn search_hints(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
    query: web::Query<SearchHintsQuery>,
) -> HttpResponse {
    let q = query.into_inner();
    let Some(term) = q.search_term.as_deref().filter(|s| !s.is_empty()) else {
        return HttpResponse::Ok().json(json!({
            "SearchHints": [],
            "TotalRecordCount": 0,
        }));
    };
    let want_artist = q
        .include_item_types
        .as_deref()
        .map(|s| {
            s.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("MusicArtist"))
        })
        .unwrap_or(true);
    let want_album = q
        .include_item_types
        .as_deref()
        .map(|s| {
            s.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("MusicAlbum"))
        })
        .unwrap_or(true);
    let want_song = q
        .include_item_types
        .as_deref()
        .map(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case("Audio")))
        .unwrap_or(true);

    let limit = q.limit.unwrap_or(20).max(1);
    let mut hints: Vec<Value> = Vec::new();

    if want_artist {
        if let Ok(rows) = repo::search_artists(&state.pool, term, limit, 0).await {
            for a in rows {
                let id = mapping::remember_artist(&state.pool, &a)
                    .await
                    .unwrap_or_else(|_| mapping::guid(mapping::KIND_ARTIST, &a.id));
                hints.push(json!({
                    "ItemId": id.clone(),
                    "Id": id,
                    "Name": a.name,
                    "Type": "MusicArtist",
                    "MediaType": "Unknown",
                    "IsFolder": true,
                }));
            }
        }
    }
    if want_album {
        if let Ok(rows) = repo::search_albums(&state.pool, term, limit, 0).await {
            for al in rows {
                let id = mapping::remember_album(&state.pool, &al)
                    .await
                    .unwrap_or_else(|_| mapping::guid(mapping::KIND_ALBUM, &al.id));
                hints.push(json!({
                    "ItemId": id.clone(),
                    "Id": id,
                    "Name": al.title,
                    "Album": al.title,
                    "AlbumArtist": al.artist,
                    "Type": "MusicAlbum",
                    "MediaType": "Unknown",
                    "IsFolder": true,
                }));
            }
        }
    }
    if want_song {
        if let Ok(rows) = repo::search_songs(&state.pool, term, limit, 0).await {
            for s in rows {
                let id = mapping::remember_song(&state.pool, &s)
                    .await
                    .unwrap_or_else(|_| mapping::guid(mapping::KIND_SONG, &s.id));
                hints.push(json!({
                    "ItemId": id.clone(),
                    "Id": id,
                    "Name": s.title,
                    "Album": s.album,
                    "AlbumArtist": s.artist,
                    "Type": "Audio",
                    "MediaType": "Audio",
                    "IsFolder": false,
                    "RunTimeTicks": s.duration_ms * TICKS_PER_MS,
                }));
            }
        }
    }

    let total = hints.len() as i32;
    HttpResponse::Ok().json(json!({
        "SearchHints": hints,
        "TotalRecordCount": total,
    }))
}

pub async fn displaypreferences(_user: AuthedUser) -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "Id": "",
        "ViewType": "",
        "SortBy": "SortName",
        "SortOrder": "Ascending",
        "RememberIndexing": false,
        "PrimaryImageHeight": 250,
        "PrimaryImageWidth": 250,
        "CustomPrefs": {},
        "ScrollDirection": "Vertical",
        "ShowBackdrop": true,
        "RememberSorting": false,
        "IndexBy": "",
        "ShowSidebar": false,
        "Client": ""
    }))
}

pub async fn branding_config() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "LoginDisclaimer": "",
        "CustomCss": "",
        "SplashscreenEnabled": false,
    }))
}
