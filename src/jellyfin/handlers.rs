use crate::models::{Album, Artist, Song, Video};
use crate::server::repo;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::auth::{self, AuthedUser, EmbyAuth};
use super::dto::{
    AuthenticationResult, BaseItemDto, ImageBlurHashes, ImageTags, ItemsResult, MediaSource,
    MediaStream, NameGuidPair, PlaybackInfoResponse, PublicSystemInfo, SessionInfoDto,
    SystemInfo, UserConfiguration, UserDto, UserItemDataDto, UserPolicy, ViewsResult,
    JELLYFIN_API_VERSION,
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
            Some(c) if c.is_ascii_lowercase() => Box::leak(
                format!("{}{}", c.to_ascii_uppercase(), &other[1..]).into_boxed_str(),
            ),
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

pub async fn system_info_public(
    state: web::Data<JellyfinState>,
    req: HttpRequest,
) -> HttpResponse {
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

pub async fn system_endpoint(
    _user: AuthedUser,
    req: HttpRequest,
) -> HttpResponse {
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
    if let Err(e) = auth::store_token(
        &state.pool,
        &token,
        state.user_id.as_str(),
        &parsed,
        &now,
    )
    .await
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

pub async fn users_list(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
) -> HttpResponse {
    HttpResponse::Ok().json(vec![build_user(&state)])
}

pub async fn users_me(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
) -> HttpResponse {
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

fn all_library_views(state: &JellyfinState) -> Vec<BaseItemDto> {
    let mut v = vec![music_library_view(state)];
    if let Some(view) = movies_library_view(state) {
        v.push(view);
    }
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

pub async fn media_folders(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
) -> HttpResponse {
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
    }
}

async fn artist_to_dto(state: &JellyfinState, a: &Artist) -> BaseItemDto {
    let id = mapping::remember_artist(&state.pool, a)
        .await
        .unwrap_or_else(|_| mapping::guid(mapping::KIND_ARTIST, &a.id));
    BaseItemDto {
        id,
        server_id: Some(state.server_id.clone()),
        name: Some(a.name.clone()),
        item_type: "MusicArtist",
        media_type: "Unknown",
        is_folder: Some(true),
        sort_name: Some(a.name.clone()),
        location_type: Some("FileSystem"),
        image_tags: Some(ImageTags {
            primary: Some(a.id.clone()),
        }),
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
    BaseItemDto {
        id: id.clone(),
        server_id: Some(state.server_id.clone()),
        name: Some(al.title.clone()),
        item_type: "MusicAlbum",
        media_type: "Unknown",
        is_folder: Some(true),
        production_year: if al.year > 0 { Some(al.year as i32) } else { None },
        premiere_date: if al.year > 0 {
            Some(format!("{:04}-01-01T00:00:00.0000000", al.year))
        } else {
            None
        },
        album: Some(al.title.clone()),
        album_id: Some(id),
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
        image_tags: Some(ImageTags {
            primary: al.cover_art.clone().map(|_| al.id.clone()),
        }),
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
        user_data: Some(UserItemDataDto {
            rating: None,
            played_percentage: None,
            unplayed_item_count: None,
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            likes: None,
            last_played_date: None,
            played: false,
            key: id.clone(),
            item_id: id,
        }),
        ..Default::default()
    }
}

async fn video_to_dto(state: &JellyfinState, v: &Video) -> BaseItemDto {
    let id = mapping::remember_video(&state.pool, v)
        .await
        .unwrap_or_else(|_| mapping::guid(mapping::KIND_VIDEO, &v.id));
    let run_time_ticks = v.duration_ms * TICKS_PER_MS;

    let video_stream = MediaStream {
        codec: Some(v.container.clone()),
        stream_type: "Video",
        index: 0,
        is_default: true,
        channels: None,
        sample_rate: None,
        bit_rate: Some(v.bitrate as i32),
        height: if v.height > 0 { Some(v.height as i32) } else { None },
        width: if v.width > 0 { Some(v.width as i32) } else { None },
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
        height: if v.height > 0 { Some(v.height as i32) } else { None },
        width: if v.width > 0 { Some(v.width as i32) } else { None },
        location_type: Some("FileSystem"),
        media_sources: Some(vec![media_source.clone()]),
        media_source_count: Some(1),
        media_streams: Some(vec![video_stream]),
        image_tags: Some(ImageTags {
            primary: v.poster_path.as_ref().map(|_| id.clone()),
        }),
        image_blur_hashes: Some(ImageBlurHashes::default()),
        user_data: Some(UserItemDataDto {
            rating: None,
            played_percentage: None,
            unplayed_item_count: None,
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            likes: None,
            last_played_date: None,
            played: false,
            key: id.clone(),
            item_id: id,
        }),
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

async fn resolve_native(
    state: &JellyfinState,
    guid: &str,
) -> Option<(String, String)> {
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

    // No parent: filter by AlbumArtistIds / ArtistIds (album or song
    // browsing for an artist — Amcfy's "artist detail" page does this).
    let artist_filter = q
        .album_artist_ids
        .clone()
        .or_else(|| q.artist_ids.clone());

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

/// `/Artists/Prefixes` — same shape as `/Items/Prefixes?IncludeItemTypes=MusicArtist`,
/// but the URL itself implies the type so we don't need query-string hints.
pub async fn artists_prefixes(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
) -> HttpResponse {
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
            } else {
                ""
            }
        })
        .unwrap_or("");

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

async fn list_artists_or_albums(
    state: &JellyfinState,
    q: &ItemsQuery,
) -> HttpResponse {
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
    if let Some(a) = artists
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(&name))
    {
        HttpResponse::Ok().json(artist_to_dto(&state, a).await)
    } else {
        HttpResponse::NotFound().finish()
    }
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

async fn stream_by_guid(
    state: &JellyfinState,
    guid: &str,
    req: &HttpRequest,
) -> HttpResponse {
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

// ── Video stream ─────────────────────────────────────────────────────────────

async fn video_by_guid(
    state: &JellyfinState,
    guid: &str,
    req: &HttpRequest,
) -> HttpResponse {
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

pub async fn sessions_playing_progress(
    _user: AuthedUser,
    _body: web::Json<Value>,
) -> HttpResponse {
    HttpResponse::NoContent().finish()
}

pub async fn sessions_playing_stopped(
    _user: AuthedUser,
    _body: web::Json<Value>,
) -> HttpResponse {
    HttpResponse::NoContent().finish()
}

pub async fn user_played_item(
    _user: AuthedUser,
    _state: web::Data<JellyfinState>,
    _path: web::Path<(String, String)>,
) -> HttpResponse {
    HttpResponse::NoContent().finish()
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
            tracing::info!(
                "jellyfin: triggered music scan of {}",
                music_dir.display()
            );
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
                tracing::info!(
                    "jellyfin: triggered video scan of {}",
                    video_dir.display()
                );
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
    let q = parse_items_query(&req);
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

/// `/UserViews?userId=...&includeHidden=...` — Findroid uses this instead of
/// the path-parametric `/Users/{id}/Views`. Same payload.
pub async fn user_views_query(
    _user: AuthedUser,
    state: web::Data<JellyfinState>,
) -> HttpResponse {
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
        .map(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case("MusicArtist")))
        .unwrap_or(true);
    let want_album = q
        .include_item_types
        .as_deref()
        .map(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case("MusicAlbum")))
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
