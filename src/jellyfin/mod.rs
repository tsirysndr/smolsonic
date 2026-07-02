pub mod auth;
pub mod discovery;
pub mod dto;
pub mod handlers;
pub mod lyrics;
pub mod mapping;
pub mod similar;

use crate::config::JellyfinConfig;
use crate::db::Db;
use crate::scanner::ScanProgress;
use crate::video_scanner::VideoScanProgress;
use actix_cors::Cors;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use std::path::PathBuf;
use std::sync::Arc;

pub struct JellyfinState {
    pub pool: Db,
    pub username: Arc<String>,
    pub password: Arc<String>,
    pub music_dir: PathBuf,
    pub covers_dir: PathBuf,
    pub server_id: String,
    pub server_name: String,
    pub user_id: Arc<String>,
    pub host: String,
    pub port: u16,
    pub video_library_name: Option<String>,
    pub video_dir: Option<PathBuf>,
    pub music_scan_progress: Arc<ScanProgress>,
    pub video_scan_progress: Arc<VideoScanProgress>,
    /// Optional Last.fm / MusicBrainz plugins for `/…/Similar`. Both slots
    /// are `None` when their `[lastfm]` / `[musicbrainz]` blocks are absent
    /// — the Similar handlers short-circuit to empty in that case.
    pub similar: Arc<similar::SimilarProviders>,
}

pub async fn start(
    cfg: JellyfinConfig,
    pool: Db,
    username: String,
    password: String,
    music_dir: PathBuf,
    covers_dir: PathBuf,
    video_library_name: Option<String>,
    video_dir: Option<PathBuf>,
    lastfm: Option<crate::config::LastfmConfig>,
    musicbrainz: Option<crate::config::MusicbrainzConfig>,
    music_scan_progress: Arc<ScanProgress>,
    video_scan_progress: Arc<VideoScanProgress>,
) -> anyhow::Result<()> {
    let server_id = auth::ensure_server_id(&pool).await?;
    let user_id = mapping::user_guid(&username);
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let similar_providers = Arc::new(similar::SimilarProviders::new(
        lastfm.as_ref(),
        musicbrainz.as_ref(),
    ));

    tracing::info!(
        "jellyfin similar plugins: lastfm={} musicbrainz={}",
        similar_providers.lastfm.is_some(),
        similar_providers.musicbrainz.is_some(),
    );

    let state = web::Data::new(JellyfinState {
        pool,
        username: Arc::new(username),
        password: Arc::new(password),
        music_dir,
        covers_dir,
        server_id: server_id.clone(),
        server_name: cfg.server_name.clone(),
        user_id: Arc::new(user_id),
        host: cfg.host.clone(),
        port: cfg.port,
        video_library_name,
        video_dir,
        music_scan_progress,
        video_scan_progress,
        similar: similar_providers,
    });

    tracing::info!(
        "starting Jellyfin API on {addr} (server={}, id={server_id})",
        cfg.server_name
    );

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .wrap(Cors::permissive())
            .configure(configure_routes)
            .default_service(web::to(log_unrouted))
    })
    .bind(&addr)?
    .run()
    .await?;

    Ok(())
}

/// Log every request that no registered route matched. Lets us see exactly
/// what URL a client (Moonfin, Findroid, etc.) hits when something appears
/// missing — `tracing::warn!` so it shows up at default RUST_LOG level.
async fn log_unrouted(req: HttpRequest) -> HttpResponse {
    tracing::warn!(
        "jellyfin: 404 {} {}{}",
        req.method(),
        req.path(),
        if req.query_string().is_empty() {
            String::new()
        } else {
            format!("?{}", req.query_string())
        },
    );
    HttpResponse::NotFound().finish()
}

/// All Jellyfin routes. Extracted so tests can mount them on an
/// `App::configure(configure_routes)` against an in-memory state.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/", web::get().to(handlers::index))
        .route("/", web::head().to(handlers::index))
        // System
        .route(
            "/System/Info/Public",
            web::get().to(handlers::system_info_public),
        )
        .route("/System/Info", web::get().to(handlers::system_info))
        .route("/System/Endpoint", web::get().to(handlers::system_endpoint))
        .route(
            "/System/Configuration/branding",
            web::get().to(handlers::branding_config),
        )
        .route(
            "/Branding/Configuration",
            web::get().to(handlers::branding_config),
        )
        .route(
            "/Branding/Css",
            web::get().to(|| async {
                actix_web::HttpResponse::Ok()
                    .content_type("text/css")
                    .body("")
            }),
        )
        // Users / auth — register both PascalCase (spec) and lowercase
        // (Amcfy Music for Android uses `/Users/authenticatebyname`).
        // Jellyfin's reference server is case-insensitive on paths; we
        // approximate that by listing the common case variants we've seen.
        .route(
            "/Users/AuthenticateByName",
            web::post().to(handlers::authenticate_by_name),
        )
        .route(
            "/Users/authenticatebyname",
            web::post().to(handlers::authenticate_by_name),
        )
        .route(
            "/Users/authenticateByName",
            web::post().to(handlers::authenticate_by_name),
        )
        .route("/Users/Public", web::get().to(handlers::users_public))
        .route("/Users", web::get().to(handlers::users_list))
        .route("/Users/Me", web::get().to(handlers::users_me))
        .route("/Users/{id}", web::get().to(handlers::user_by_id))
        // Views / libraries
        .route("/Users/{id}/Views", web::get().to(handlers::user_views))
        .route(
            "/Library/MediaFolders",
            web::get().to(handlers::media_folders),
        )
        .route(
            "/Library/VirtualFolders",
            web::get().to(handlers::library_virtual_folders),
        )
        // Items — specific paths MUST be registered before /Items/{id}
        // because actix matches in registration order.
        .route("/Items", web::get().to(handlers::items))
        .route("/Items/Suggestions", web::get().to(handlers::empty_items))
        .route("/Items/Resume", web::get().to(handlers::empty_items))
        .route("/Items/Latest", web::get().to(handlers::items_latest))
        .route("/Items/Prefixes", web::get().to(handlers::items_prefixes))
        // Movie-detail rails Moonfin probes. We have no extras / similar /
        // intros / chapter markers, so empty results are correct — but they
        // must be ROUTED so they don't show up in the unrouted-404 log.
        .route(
            "/Items/{id}/SpecialFeatures",
            web::get().to(handlers::empty_array),
        )
        .route(
            "/Items/{id}/Ancestors",
            web::get().to(handlers::empty_array),
        )
        // Similar — modelled after the Jellyfin OpenAPI Library tag.
        // Powered by the Last.fm / MusicBrainz plugins when enabled;
        // returns empty results when neither token is configured. The
        // per-kind paths MUST come before their parent catch-alls
        // (/Items/{id}, /Artists/{name}, /Playlists/{id}).
        .route("/Items/{id}/Similar", web::get().to(handlers::item_similar))
        .route(
            "/Users/{uid}/Items/{id}/Intros",
            web::get().to(handlers::empty_items),
        )
        .route("/MediaSegments/{id}", web::get().to(handlers::empty_items))
        // Finamp / just_audio stream the original file via this endpoint,
        // passing the token as `?ApiKey=`. Routed before /Items/{id} so the
        // path parameter doesn't capture "{id}/File".
        .route(
            "/Items/{id}/File",
            web::get().to(handlers::item_file_stream),
        )
        .route(
            "/Items/{id}/File",
            web::head().to(handlers::item_file_stream),
        )
        .route(
            "/Items/{id}/Download",
            web::get().to(handlers::item_file_stream),
        )
        // InstantMix — modelled after the Jellyfin OpenAPI InstantMix tag.
        // Every route must precede its parent catch-all (`/Items/{id}`,
        // `/Artists/{name}`, `/Playlists/{id}`) because actix matches in
        // registration order.
        .route(
            "/Items/{id}/InstantMix",
            web::get().to(handlers::item_instant_mix),
        )
        // RemoteImage — modelled after the Jellyfin OpenAPI RemoteImage tag.
        // Powered by the Last.fm / MusicBrainz plugins; empty when neither
        // is enabled. `/RemoteImages/Providers` and `/Download` must
        // precede the plain `/RemoteImages` so actix binds the longer
        // paths first.
        .route(
            "/Items/{id}/RemoteImages/Providers",
            web::get().to(handlers::remote_image_providers),
        )
        .route(
            "/Items/{id}/RemoteImages/Download",
            web::post().to(handlers::download_remote_image),
        )
        .route(
            "/Items/{id}/RemoteImages",
            web::get().to(handlers::remote_images),
        )
        .route("/Items/{id}", web::get().to(handlers::item_by_id))
        // DELETE /Items/{id} — Jellyfin uses this to delete playlists (they
        // are `BaseItem`s). Songs/albums are read-only in smolsonic and get
        // 403 here.
        .route("/Items/{id}", web::delete().to(handlers::delete_item))
        .route("/Users/{id}/Items", web::get().to(handlers::user_items))
        .route(
            "/Users/{id}/Items/Resume",
            web::get().to(handlers::empty_items),
        )
        .route(
            "/Users/{id}/Items/Latest",
            web::get().to(handlers::empty_array),
        )
        .route(
            "/Users/{uid}/Items/{id}",
            web::get().to(handlers::user_item_by_id),
        )
        // Legacy /UserItems/* aliases — Findroid hits these for the home rails.
        .route("/UserItems/Resume", web::get().to(handlers::empty_items))
        .route("/UserItems/Latest", web::get().to(handlers::empty_array))
        // /UserViews?userId=... — Findroid uses this instead of /Users/{id}/Views.
        .route("/UserViews", web::get().to(handlers::user_views_query))
        // Search endpoints — backed by the existing FTS in repo.rs.
        .route("/Search/Hints", web::get().to(handlers::search_hints))
        // /ScheduledTasks/* — Amcfy and the official client probe these to
        // trigger library scans / probes. smolsonic has its own scanner; we
        // ack the trigger but don't expose any task state.
        .route("/ScheduledTasks", web::get().to(handlers::empty_array))
        .route(
            "/ScheduledTasks/Running/{id}",
            web::post().to(handlers::trigger_library_scan),
        )
        .route(
            "/ScheduledTasks/Running/{id}",
            web::delete().to(handlers::no_content),
        )
        .route(
            "/ScheduledTasks/{id}/Triggers",
            web::post().to(handlers::no_content),
        )
        .route(
            "/Library/Refresh",
            web::post().to(handlers::trigger_library_scan),
        )
        // /Shows/* — TV-series endpoints. smolsonic has no series concept,
        // so empty results are the right answer.
        .route("/Shows/NextUp", web::get().to(handlers::empty_items))
        .route("/Shows/Upcoming", web::get().to(handlers::empty_items))
        .route("/Shows/{id}/Episodes", web::get().to(handlers::empty_items))
        .route("/Shows/{id}/Seasons", web::get().to(handlers::empty_items))
        .route(
            "/Shows/{id}/Similar",
            web::get().to(handlers::video_similar),
        )
        .route(
            "/Movies/{id}/Similar",
            web::get().to(handlers::video_similar),
        )
        .route(
            "/Trailers/{id}/Similar",
            web::get().to(handlers::video_similar),
        )
        // Artists
        .route("/Artists", web::get().to(handlers::artists))
        .route("/Artists/AlbumArtists", web::get().to(handlers::artists))
        // Some clients ask for the artist alpha-jump rail via `/Artists/Prefixes`
        // instead of `/Items/Prefixes?IncludeItemTypes=MusicArtist`. Same data.
        .route(
            "/Artists/Prefixes",
            web::get().to(handlers::artists_prefixes),
        )
        // `/Artists/{id}/InstantMix` MUST come before `/Artists/{name}`
        // (both share the same segment count so actix falls back to reg
        // order for disambiguation).
        .route(
            "/Artists/{id}/InstantMix",
            web::get().to(handlers::artist_instant_mix),
        )
        .route(
            "/Artists/{id}/Similar",
            web::get().to(handlers::artist_similar),
        )
        .route(
            "/Albums/{id}/InstantMix",
            web::get().to(handlers::album_instant_mix),
        )
        .route(
            "/Albums/{id}/Similar",
            web::get().to(handlers::album_similar),
        )
        .route(
            "/Songs/{id}/InstantMix",
            web::get().to(handlers::song_instant_mix),
        )
        // `/MusicGenres/InstantMix` (literal) must precede
        // `/MusicGenres/{name}/InstantMix` so the query-id form binds first.
        .route(
            "/MusicGenres/InstantMix",
            web::get().to(handlers::genre_instant_mix_by_id),
        )
        .route(
            "/MusicGenres/{name}/InstantMix",
            web::get().to(handlers::genre_instant_mix),
        )
        .route("/Artists/{name}", web::get().to(handlers::artist_by_name))
        // Images — Findroid uses lowercase `/items/...` so we register both.
        .route(
            "/Items/{id}/Images/{kind}",
            web::get().to(handlers::item_image),
        )
        .route(
            "/Items/{id}/Images/{kind}/{idx}",
            web::get().to(handlers::item_image_by_index),
        )
        .route(
            "/items/{id}/Images/{kind}",
            web::get().to(handlers::item_image),
        )
        .route(
            "/items/{id}/Images/{kind}/{idx}",
            web::get().to(handlers::item_image_by_index),
        )
        .route(
            "/Items/{id}/Images/{kind}",
            web::head().to(handlers::item_image),
        )
        .route(
            "/items/{id}/Images/{kind}",
            web::head().to(handlers::item_image),
        )
        // Playback
        .route(
            "/Items/{id}/PlaybackInfo",
            web::get().to(handlers::playback_info),
        )
        .route(
            "/Items/{id}/PlaybackInfo",
            web::post().to(handlers::playback_info),
        )
        .route("/Audio/{id}/stream", web::get().to(handlers::audio_stream))
        .route("/Audio/{id}/stream", web::head().to(handlers::audio_stream))
        .route(
            "/Audio/{id}/stream.{ext}",
            web::get().to(handlers::audio_stream_ext),
        )
        .route(
            "/Audio/{id}/stream.{ext}",
            web::head().to(handlers::audio_stream_ext),
        )
        .route(
            "/Audio/{id}/universal",
            web::get().to(handlers::audio_universal),
        )
        .route(
            "/Audio/{id}/universal",
            web::head().to(handlers::audio_universal),
        )
        // Lyric — modelled after the Jellyfin OpenAPI Lyric tag. The
        // `/RemoteSearch` variants must precede the plain `/Audio/{id}/Lyrics`
        // so actix binds the longer path first.
        .route(
            "/Audio/{id}/RemoteSearch/Lyrics/{lyricId}",
            web::post().to(handlers::download_remote_lyric),
        )
        .route(
            "/Audio/{id}/RemoteSearch/Lyrics",
            web::get().to(handlers::remote_search_lyrics),
        )
        .route("/Audio/{id}/Lyrics", web::get().to(handlers::get_lyrics))
        .route(
            "/Audio/{id}/Lyrics",
            web::post().to(handlers::upload_lyrics),
        )
        .route(
            "/Audio/{id}/Lyrics",
            web::delete().to(handlers::delete_lyrics),
        )
        .route(
            "/Providers/Lyrics/{lyricId}",
            web::get().to(handlers::get_remote_lyric),
        )
        // Video stream
        .route("/Videos/{id}/stream", web::get().to(handlers::video_stream))
        .route(
            "/Videos/{id}/stream",
            web::head().to(handlers::video_stream),
        )
        .route(
            "/Videos/{id}/stream.{ext}",
            web::get().to(handlers::video_stream_ext),
        )
        .route(
            "/Videos/{id}/stream.{ext}",
            web::head().to(handlers::video_stream_ext),
        )
        // Sessions / scrobble
        .route("/Sessions", web::get().to(handlers::sessions_list))
        .route(
            "/Sessions/Capabilities/Full",
            web::post().to(handlers::sessions_capabilities),
        )
        .route(
            "/Sessions/Playing",
            web::post().to(handlers::sessions_playing),
        )
        .route(
            "/Sessions/Playing/Progress",
            web::post().to(handlers::sessions_playing_progress),
        )
        .route(
            "/Sessions/Playing/Stopped",
            web::post().to(handlers::sessions_playing_stopped),
        )
        // UserData / PlayedItems / Rating — modelled after the Jellyfin
        // OpenAPI UserLibrary tag. Spec forms use no {userId} (smolsonic is
        // single-user); legacy per-user forms are registered because older
        // clients (Symfonium, Finamp <=1.9) still hit them.
        .route(
            "/UserItems/{id}/UserData",
            web::get().to(handlers::get_user_item_data_endpoint),
        )
        .route(
            "/UserItems/{id}/UserData",
            web::post().to(handlers::update_user_item_data_endpoint),
        )
        .route(
            "/Users/{uid}/Items/{id}/UserData",
            web::get().to(handlers::get_user_item_data_legacy),
        )
        .route(
            "/Users/{uid}/Items/{id}/UserData",
            web::post().to(handlers::update_user_item_data_legacy),
        )
        .route(
            "/UserPlayedItems/{id}",
            web::post().to(handlers::mark_played_endpoint),
        )
        .route(
            "/UserPlayedItems/{id}",
            web::delete().to(handlers::mark_unplayed_endpoint),
        )
        .route(
            "/Users/{uid}/PlayedItems/{id}",
            web::post().to(handlers::mark_played_legacy),
        )
        .route(
            "/Users/{uid}/PlayedItems/{id}",
            web::delete().to(handlers::mark_unplayed_legacy),
        )
        .route(
            "/UserItems/{id}/Rating",
            web::post().to(handlers::set_rating_endpoint),
        )
        .route(
            "/UserItems/{id}/Rating",
            web::delete().to(handlers::clear_rating_endpoint),
        )
        .route(
            "/Users/{uid}/Items/{id}/Rating",
            web::post().to(handlers::set_rating_legacy),
        )
        .route(
            "/Users/{uid}/Items/{id}/Rating",
            web::delete().to(handlers::clear_rating_legacy),
        )
        // Favorites — modelled after the Jellyfin OpenAPI UserLibrary tag.
        // Both the spec form (`/UserFavoriteItems/{id}`) and the legacy
        // per-user form (`/Users/{uid}/FavoriteItems/{id}`) are registered
        // because in-the-wild clients (Findroid, Streamyfin, Moonfin, Amcfy)
        // still hit both.
        .route(
            "/UserFavoriteItems/{id}",
            web::post().to(handlers::add_favorite_item),
        )
        .route(
            "/UserFavoriteItems/{id}",
            web::delete().to(handlers::remove_favorite_item),
        )
        .route(
            "/Users/{uid}/FavoriteItems/{id}",
            web::post().to(handlers::add_user_favorite_item),
        )
        .route(
            "/Users/{uid}/FavoriteItems/{id}",
            web::delete().to(handlers::remove_user_favorite_item),
        )
        // Common probes that clients hit — answer empty so they stop retrying.
        .route(
            "/DisplayPreferences/{id}",
            web::get().to(handlers::displaypreferences),
        )
        // Playlists — modelled after the Jellyfin OpenAPI Playlists tag.
        // Order matters: specific `/Playlists/{id}/…` paths must precede the
        // catch-all `/Playlists/{id}` GET.
        .route("/Playlists", web::get().to(handlers::playlists_list))
        .route(
            "/Playlists",
            web::post().to(handlers::create_playlist_endpoint),
        )
        .route(
            "/Playlists/{id}/Items",
            web::get().to(handlers::playlist_items),
        )
        .route(
            "/Playlists/{id}/Items",
            web::post().to(handlers::add_playlist_items),
        )
        .route(
            "/Playlists/{id}/Items",
            web::delete().to(handlers::remove_playlist_items),
        )
        .route(
            "/Playlists/{id}/Items/{item_id}/Move/{new_index}",
            web::post().to(handlers::move_playlist_item),
        )
        .route(
            "/Playlists/{id}/Users",
            web::get().to(handlers::playlist_users),
        )
        .route(
            "/Playlists/{id}/InstantMix",
            web::get().to(handlers::playlist_instant_mix),
        )
        .route(
            "/Playlists/{id}",
            web::get().to(handlers::get_playlist_endpoint),
        )
        .route(
            "/Playlists/{id}",
            web::post().to(handlers::update_playlist_endpoint),
        )
        .route(
            "/Users/{id}/Items/Suggestions",
            web::get().to(handlers::empty_items),
        )
        .route(
            "/Users/{id}/Views/{view}/Latest",
            web::get().to(handlers::empty_array),
        )
        .route("/Genres", web::get().to(handlers::empty_items))
        .route("/MusicGenres", web::get().to(handlers::empty_items))
        // /System/Ping is the canonical Jellyfin heartbeat — plain text body.
        .route("/System/Ping", web::get().to(handlers::system_ping))
        .route("/System/Ping", web::head().to(handlers::system_ping))
        // Endpoints we deliberately 404 but want routed (no log noise):
        //  - /socket: WebSocket live updates; clients fall back to polling
        //  - /Moonfin/Ping: Moonfin's own client-side probe (not in spec)
        //  - /Users/{id}/Images/*: no user avatars stored
        .route("/socket", web::get().to(handlers::not_found))
        .route("/Moonfin/Ping", web::get().to(handlers::not_found))
        .route(
            "/Users/{id}/Images/{kind}",
            web::get().to(handlers::not_found),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use actix_web::{http::StatusCode, test, App};
    use serde_json::Value;
    use std::io::Write;

    async fn fixture_state(
        music_dir: &std::path::Path,
        covers_dir: &std::path::Path,
        with_video: bool,
    ) -> JellyfinState {
        // On-disk sqlite so all migrations and FTS triggers actually run.
        let db_path = music_dir.join("test.db");
        let pool = db::init(&db_path).await.unwrap();

        // Insert one artist, one album, one song pointing at a real file in music_dir.
        let song_path = music_dir.join("song.mp3");
        let mut f = std::fs::File::create(&song_path).unwrap();
        f.write_all(&[0u8; 4096]).unwrap();

        sqlx::query("INSERT INTO artists (id, name, name_lower) VALUES ('ar-1','Test Artist','test artist')")
            .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO albums (id, title, artist, artist_id, year, cover_art)
             VALUES ('al-1','Test Album','Test Artist','ar-1',2020,NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO songs (id, path, title, artist, artist_id, album, album_id, genre,
                track_number, disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art, mtime)
             VALUES ('so-1', ?1, 'Test Song', 'Test Artist', 'ar-1', 'Test Album', 'al-1', NULL,
                1, 1, 2020, 60000, 192, 4096, 'mp3', 'audio/mpeg', NULL, 0)",
        )
        .bind(song_path.to_string_lossy().to_string())
        .execute(&pool).await.unwrap();

        if with_video {
            let video_path = music_dir.join("movie.mp4");
            let mut vf = std::fs::File::create(&video_path).unwrap();
            vf.write_all(&[0u8; 8192]).unwrap();
            sqlx::query(
                "INSERT INTO videos (id, path, title, container, duration_ms, filesize,
                    bitrate, width, height, poster_path, mtime)
                 VALUES ('vi-1', ?1, 'Test Movie', 'mp4', 90000000, 8192,
                    2_000_000, 1920, 1080, NULL, 0)",
            )
            .bind(video_path.to_string_lossy().to_string())
            .execute(&pool)
            .await
            .unwrap();
        }

        let server_id = auth::ensure_server_id(&pool).await.unwrap();
        let user_id = mapping::user_guid("alice");
        JellyfinState {
            pool,
            username: Arc::new("alice".to_string()),
            password: Arc::new("secret".to_string()),
            music_dir: music_dir.to_path_buf(),
            covers_dir: covers_dir.to_path_buf(),
            server_id,
            server_name: "test".to_string(),
            user_id: Arc::new(user_id),
            host: "127.0.0.1".to_string(),
            port: 0,
            video_library_name: if with_video {
                Some("Movies".to_string())
            } else {
                None
            },
            video_dir: if with_video {
                Some(music_dir.to_path_buf())
            } else {
                None
            },
            music_scan_progress: Arc::new(ScanProgress::default()),
            video_scan_progress: Arc::new(VideoScanProgress::default()),
            similar: Arc::new(similar::SimilarProviders::new(None, None)),
        }
    }

    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("smolsonic-jf-test-{}", std::process::id()));
        // Ensure a fresh dir per test by appending a random suffix.
        let unique = base.join(auth::random_hex(8));
        std::fs::create_dir_all(&unique).unwrap();
        unique
    }

    #[actix_web::test]
    async fn system_info_public_unauthenticated() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;
        let server_id = state.server_id.clone();
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/System/Info/Public")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body["Id"], server_id);
        assert_eq!(body["ServerName"], "test");
    }

    #[actix_web::test]
    async fn authenticate_then_list_artists_then_stream() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        // Wrong password → 401.
        let bad = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"nope"}))
            .to_request();
        assert_eq!(
            test::call_service(&app, bad).await.status(),
            StatusCode::UNAUTHORIZED
        );

        // Correct credentials → token.
        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();
        assert!(!token.is_empty());

        // Protected endpoint without token → 401.
        let no_auth = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicArtist")
            .to_request();
        assert_eq!(
            test::call_service(&app, no_auth).await.status(),
            StatusCode::UNAUTHORIZED
        );

        // List artists.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicArtist")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let items: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(items["TotalRecordCount"], 1);
        let artist_id = items["Items"][0]["Id"].as_str().unwrap().to_string();
        assert_eq!(items["Items"][0]["Name"], "Test Artist");

        // Alpha-jump filter applies to artists / albums / songs alike. The
        // fixture has "Test Artist" / "Test Album" / "Test Song" — all under
        // T. So `NameStartsWith=T` returns 1 and `NameStartsWith=X` returns 0
        // for each item type, including via the dedicated `/Artists` route.
        for (uri, expected_t) in [
            ("/Items?IncludeItemTypes=MusicArtist&NameStartsWith=T", 1),
            ("/Items?IncludeItemTypes=MusicArtist&NameStartsWith=X", 0),
            ("/Items?IncludeItemTypes=MusicAlbum&NameStartsWith=T", 1),
            ("/Items?IncludeItemTypes=MusicAlbum&NameStartsWith=X", 0),
            ("/Items?IncludeItemTypes=Audio&NameStartsWith=T", 1),
            ("/Items?IncludeItemTypes=Audio&NameStartsWith=X", 0),
            ("/Artists?NameStartsWith=T", 1),
            ("/Artists?NameStartsWith=X", 0),
        ] {
            let req = test::TestRequest::get()
                .uri(uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            let resp: Value = test::call_and_read_body_json(&app, req).await;
            assert_eq!(
                resp["TotalRecordCount"], expected_t,
                "wrong count for {uri}"
            );
        }

        // /Artists/Prefixes returns ["T"] for the fixture.
        let req = test::TestRequest::get()
            .uri("/Artists/Prefixes")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let prefixes: Value = test::call_and_read_body_json(&app, req).await;
        let names: Vec<&str> = prefixes
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["Name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["T"]);

        // /Items/Prefixes?IncludeItemTypes=MusicAlbum returns ["T"] too.
        let req = test::TestRequest::get()
            .uri("/Items/Prefixes?IncludeItemTypes=MusicAlbum")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let prefixes: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(prefixes[0]["Name"], "T");

        // List albums under that artist.
        let req = test::TestRequest::get()
            .uri(&format!("/Items?ParentId={artist_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let albums: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(albums["TotalRecordCount"], 1);
        let album_id = albums["Items"][0]["Id"].as_str().unwrap().to_string();
        assert_eq!(albums["Items"][0]["Name"], "Test Album");

        // List songs under that album.
        let req = test::TestRequest::get()
            .uri(&format!("/Items?ParentId={album_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let songs: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(songs["TotalRecordCount"], 1);
        let song_id = songs["Items"][0]["Id"].as_str().unwrap().to_string();
        assert_eq!(songs["Items"][0]["Name"], "Test Song");
        assert_eq!(songs["Items"][0]["MediaType"], "Audio");

        // Stream the song with api_key query (HEAD).
        let req = test::TestRequest::default()
            .method(actix_web::http::Method::HEAD)
            .uri(&format!("/Audio/{song_id}/stream?api_key={token}"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(
            headers.get("content-type").unwrap().to_str().unwrap(),
            "audio/mpeg"
        );

        // Range request — first 100 bytes.
        let req = test::TestRequest::get()
            .uri(&format!("/Audio/{song_id}/stream?api_key={token}"))
            .insert_header(("Range", "bytes=0-99"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        let cl = resp
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cl, "100");
        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cr.starts_with("bytes 0-99/"));

        // Streaming without a token → 401.
        let unauth = test::TestRequest::get()
            .uri(&format!("/Audio/{song_id}/stream"))
            .to_request();
        assert_eq!(
            test::call_service(&app, unauth).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[actix_web::test]
    async fn video_library_visible_and_streamable() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, true).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        // Authenticate.
        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();

        // Views: should include Music, Movies, and Playlists.
        let req = test::TestRequest::get()
            .uri("/Users/me/Views")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let views: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(views["TotalRecordCount"], 3);
        let names: Vec<&str> = views["Items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["Name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"Music"));
        assert!(names.contains(&"Movies"));
        assert!(names.contains(&"Playlists"));

        // List movies by type.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Movie")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let movies: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(movies["TotalRecordCount"], 1);
        let movie = &movies["Items"][0];
        assert_eq!(movie["Name"], "Test Movie");
        assert_eq!(movie["Type"], "Movie");
        assert_eq!(movie["MediaType"], "Video");
        let video_id = movie["Id"].as_str().unwrap().to_string();

        // Same list via `IncludeItemTypes=Video` — some SDK consumers use the
        // BaseItemKind enum value `Video` instead of `Movie`. Must NOT fall
        // through to the artist list.
        for uri in [
            "/Items?IncludeItemTypes=Video",
            "/Items?MediaTypes=Video",
            "/Users/me/Items?IncludeItemTypes=Video&Recursive=true",
        ] {
            let req = test::TestRequest::get()
                .uri(uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            let resp: Value = test::call_and_read_body_json(&app, req).await;
            assert_eq!(resp["TotalRecordCount"], 1, "wrong count for {uri}");
            assert_eq!(resp["Items"][0]["Type"], "Movie", "wrong type for {uri}");
        }

        // Alpha-jump rail: `?NameStartsWith=T` should narrow to titles
        // starting with T (case-insensitive). The fixture inserts one movie
        // "Test Movie" — "T" matches, "X" doesn't.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Movie&NameStartsWith=T")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let resp: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(resp["TotalRecordCount"], 1);
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Movie&NameStartsWith=X")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let resp: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(resp["TotalRecordCount"], 0);

        // /Items/Prefixes should report the letter "T" for the Movies lib.
        let req = test::TestRequest::get()
            .uri(&format!(
                "/Items/Prefixes?ParentId={}",
                mapping::movies_library_guid()
            ))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let prefixes: Value = test::call_and_read_body_json(&app, req).await;
        let names: Vec<&str> = prefixes
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["Name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["T"]);

        // Moonfin taps the Movies library tile, which first fetches the
        // library item itself as `/Users/{uid}/Items/{library_guid}`. Must
        // return the CollectionFolder DTO, not 404.
        let movies_lib = mapping::movies_library_guid();
        let req = test::TestRequest::get()
            .uri(&format!("/Users/me/Items/{movies_lib}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body["Type"], "CollectionFolder");
        assert_eq!(body["CollectionType"], "movies");
        assert_eq!(body["Name"], "Movies");

        // HEAD on the video stream — direct play.
        let req = test::TestRequest::default()
            .method(actix_web::http::Method::HEAD)
            .uri(&format!("/Videos/{video_id}/stream?api_key={token}"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "video/mp4"
        );

        // Range request — first 256 bytes of the video.
        let req = test::TestRequest::get()
            .uri(&format!("/Videos/{video_id}/stream.mp4?api_key={token}"))
            .insert_header(("Range", "bytes=0-255"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers()
                .get("content-length")
                .unwrap()
                .to_str()
                .unwrap(),
            "256"
        );

        // PlaybackInfo on a video returns its media source.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{video_id}/PlaybackInfo"))
            .insert_header(("X-Emby-Token", token))
            .to_request();
        let pb: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(pb["MediaSources"][0]["Container"], "mp4");
    }

    /// End-to-end for the Playlists API modelled after the Jellyfin OpenAPI
    /// spec: create → list → get → add items → move → remove → delete.
    #[actix_web::test]
    async fn playlist_crud_roundtrip() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;
        // Insert a second song so we can test add / move / remove.
        let song2_path = dir.join("song2.mp3");
        let mut f = std::fs::File::create(&song2_path).unwrap();
        Write::write_all(&mut f, &[0u8; 4096]).unwrap();
        sqlx::query(
            "INSERT INTO songs (id, path, title, artist, artist_id, album, album_id, genre,
                track_number, disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art, mtime)
             VALUES ('so-2', ?1, 'Second Song', 'Test Artist', 'ar-1', 'Test Album', 'al-1', NULL,
                2, 1, 2020, 45000, 192, 4096, 'mp3', 'audio/mpeg', NULL, 0)",
        )
        .bind(song2_path.to_string_lossy().to_string())
        .execute(&state.pool)
        .await
        .unwrap();

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        // Authenticate.
        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();

        // Discover song GUIDs (populates jf_guids as a side effect). The list
        // comes back ordered by title (COLLATE NOCASE) — pick each song by
        // name so the test stays stable regardless of ordering.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Audio")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let songs: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(songs["TotalRecordCount"], 2);
        let by_name: std::collections::HashMap<String, String> = songs["Items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["Name"].as_str().unwrap().to_string(),
                    s["Id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let s1 = by_name["Test Song"].clone();
        let s2 = by_name["Second Song"].clone();

        // Create a playlist with one initial song via JSON body.
        let req = test::TestRequest::post()
            .uri("/Playlists")
            .insert_header(("X-Emby-Token", token.clone()))
            .set_json(serde_json::json!({
                "Name": "My Mix",
                "Ids": [s1.clone()],
                "MediaType": "Audio",
                "IsPublic": true,
            }))
            .to_request();
        let created: Value = test::call_and_read_body_json(&app, req).await;
        let playlist_id = created["Id"].as_str().unwrap().to_string();
        assert!(!playlist_id.is_empty());

        // GET single playlist DTO.
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let pl: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(pl["Name"], "My Mix");
        assert_eq!(pl["Type"], "Playlist");
        assert_eq!(pl["ChildCount"], 1);

        // Playlists surface through /Items?IncludeItemTypes=Playlist.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Playlist")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let list: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(list["TotalRecordCount"], 1);
        assert_eq!(list["Items"][0]["Id"], playlist_id);

        // Playlists library is one of the top-level views.
        let req = test::TestRequest::get()
            .uri("/Users/me/Views")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let views: Value = test::call_and_read_body_json(&app, req).await;
        let names: Vec<&str> = views["Items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["Name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"Playlists"));
        let playlists_lib = mapping::playlists_library_guid();

        // GET /Users/{uid}/Items/{playlists_lib} returns the CollectionFolder
        // header — Moonfin fetches this when the tile is tapped.
        let req = test::TestRequest::get()
            .uri(&format!("/Users/me/Items/{playlists_lib}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body["Type"], "CollectionFolder");
        assert_eq!(body["CollectionType"], "playlists");
        assert_eq!(body["Name"], "Playlists");

        // /Items?parentId=<playlists_lib> lists the playlists.
        let req = test::TestRequest::get()
            .uri(&format!("/Items?parentId={playlists_lib}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let list: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(list["TotalRecordCount"], 1);
        assert_eq!(list["Items"][0]["Id"], playlist_id);

        // GET /Playlists is our stub over the same list.
        let req = test::TestRequest::get()
            .uri("/Playlists")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let list: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(list["TotalRecordCount"], 1);

        // Append the second song.
        let req = test::TestRequest::post()
            .uri(&format!("/Playlists/{playlist_id}/Items?ids={s2}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NO_CONTENT
        );

        // Verify order: song1, song2 — capture PlaylistItemIds.
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}/Items"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let items: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(items["TotalRecordCount"], 2);
        assert_eq!(items["Items"][0]["Name"], "Test Song");
        assert_eq!(items["Items"][1]["Name"], "Second Song");
        let entry_at_0 = items["Items"][0]["PlaylistItemId"]
            .as_str()
            .unwrap()
            .to_string();
        let entry_at_1 = items["Items"][1]["PlaylistItemId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(entry_at_0, entry_at_1);

        // Move entry at position 0 → position 1. Order becomes song2, song1.
        let req = test::TestRequest::post()
            .uri(&format!(
                "/Playlists/{playlist_id}/Items/{entry_at_0}/Move/1"
            ))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NO_CONTENT
        );
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}/Items"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let items: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(items["Items"][0]["Name"], "Second Song");
        assert_eq!(items["Items"][1]["Name"], "Test Song");

        // Update: rename via POST /Playlists/{id}.
        let req = test::TestRequest::post()
            .uri(&format!("/Playlists/{playlist_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .set_json(serde_json::json!({"Name": "Renamed Mix"}))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NO_CONTENT
        );
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let pl: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(pl["Name"], "Renamed Mix");

        // Remove entry at (new) position 0 → only song1 remains.
        let items: Value = {
            let req = test::TestRequest::get()
                .uri(&format!("/Playlists/{playlist_id}/Items"))
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            test::call_and_read_body_json(&app, req).await
        };
        let head_entry = items["Items"][0]["PlaylistItemId"]
            .as_str()
            .unwrap()
            .to_string();
        let req = test::TestRequest::delete()
            .uri(&format!(
                "/Playlists/{playlist_id}/Items?entryIds={head_entry}"
            ))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NO_CONTENT
        );
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}/Items"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let items: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(items["TotalRecordCount"], 1);
        assert_eq!(items["Items"][0]["Name"], "Test Song");

        // /Playlists/{id}/Users is an empty list (single-user server).
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}/Users"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let users: Value = test::call_and_read_body_json(&app, req).await;
        assert!(users.as_array().unwrap().is_empty());

        // Delete via DELETE /Items/{id}. Non-playlist items reject.
        let req = test::TestRequest::delete()
            .uri(&format!("/Items/{s1}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::FORBIDDEN
        );
        let req = test::TestRequest::delete()
            .uri(&format!("/Items/{playlist_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NO_CONTENT
        );

        // Gone.
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}"))
            .insert_header(("X-Emby-Token", token))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    /// Exercises the full Favorites API surface modelled after the Jellyfin
    /// OpenAPI spec (UserLibrary tag + `?isFavorite` / `?Filters=IsFavorite`
    /// on `/Items` and `/Artists`).
    #[actix_web::test]
    async fn favorites_roundtrip_via_userfavoriteitems_and_filters() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        // Authenticate.
        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();
        let user_id = auth_body["User"]["Id"].as_str().unwrap().to_string();

        // Discover the song's Jellyfin GUID (populates jf_guids as a side effect).
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Audio")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let songs: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(songs["TotalRecordCount"], 1);
        let song_id = songs["Items"][0]["Id"].as_str().unwrap().to_string();
        // Fresh song → IsFavorite starts false.
        assert_eq!(songs["Items"][0]["UserData"]["IsFavorite"], false);

        // Discover the artist / album GUIDs the same way.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicArtist")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let artists: Value = test::call_and_read_body_json(&app, req).await;
        let artist_id = artists["Items"][0]["Id"].as_str().unwrap().to_string();

        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicAlbum")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let albums: Value = test::call_and_read_body_json(&app, req).await;
        let album_id = albums["Items"][0]["Id"].as_str().unwrap().to_string();

        // POST /UserFavoriteItems/{songId} — spec-compliant endpoint.
        let req = test::TestRequest::post()
            .uri(&format!("/UserFavoriteItems/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let user_data: Value = test::read_body_json(resp).await;
        // Per spec `UserItemDataDto.IsFavorite` and `ItemId`/`Key` are required.
        assert_eq!(user_data["IsFavorite"], true);
        assert_eq!(user_data["ItemId"], song_id);
        assert_eq!(user_data["Key"], song_id);

        // The item's user_data now reflects the favorite state.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let item: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(item["UserData"]["IsFavorite"], true);

        // Also via the per-user detail path (`/Users/{uid}/Items/{id}`).
        let req = test::TestRequest::get()
            .uri(&format!("/Users/{user_id}/Items/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let item: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(item["UserData"]["IsFavorite"], true);

        // Legacy per-user endpoint — POST on the album should also stick.
        let req = test::TestRequest::post()
            .uri(&format!("/Users/{user_id}/FavoriteItems/{album_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let user_data: Value = test::read_body_json(resp).await;
        assert_eq!(user_data["IsFavorite"], true);
        assert_eq!(user_data["ItemId"], album_id);

        // Favor the artist so `/Artists?isFavorite=true` returns something.
        let req = test::TestRequest::post()
            .uri(&format!("/UserFavoriteItems/{artist_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(test::call_service(&app, req).await.status(), StatusCode::OK);

        // `/Items?Filters=IsFavorite` returns the union across types (artist,
        // album, song). Total = 3 for our fixture.
        let req = test::TestRequest::get()
            .uri("/Items?Filters=IsFavorite&Recursive=true")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let favs: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(favs["TotalRecordCount"], 3);

        // `/Items?IsFavorite=true&IncludeItemTypes=Audio` filters to only songs.
        let req = test::TestRequest::get()
            .uri("/Items?IsFavorite=true&IncludeItemTypes=Audio")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let favs: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(favs["TotalRecordCount"], 1);
        assert_eq!(favs["Items"][0]["Id"], song_id);
        assert_eq!(favs["Items"][0]["Type"], "Audio");

        // `/Artists?isFavorite=true` — spec-level filter directly on /Artists.
        let req = test::TestRequest::get()
            .uri("/Artists?isFavorite=true")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let favs: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(favs["TotalRecordCount"], 1);
        assert_eq!(favs["Items"][0]["Id"], artist_id);

        // DELETE /UserFavoriteItems/{songId} unstars it.
        let req = test::TestRequest::delete()
            .uri(&format!("/UserFavoriteItems/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let user_data: Value = test::read_body_json(resp).await;
        assert_eq!(user_data["IsFavorite"], false);
        assert_eq!(user_data["ItemId"], song_id);

        // Detail now reflects the removal.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let item: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(item["UserData"]["IsFavorite"], false);

        // Filter now returns only the artist + album (song no longer starred).
        let req = test::TestRequest::get()
            .uri("/Items?Filters=IsFavorite&Recursive=true")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let favs: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(favs["TotalRecordCount"], 2);

        // POST on an unknown GUID → 404.
        let req = test::TestRequest::post()
            .uri("/UserFavoriteItems/00000000-0000-0000-0000-000000000000")
            .insert_header(("X-Emby-Token", token))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    /// Exercises the full UserData API surface modelled after the Jellyfin
    /// OpenAPI spec (UserLibrary tag): GET/POST /UserItems/{id}/UserData,
    /// POST+DELETE /UserPlayedItems/{id}, POST+DELETE /UserItems/{id}/Rating,
    /// plus the legacy per-user variants under /Users/{userId}/…
    #[actix_web::test]
    async fn user_data_roundtrip_played_rating_and_full_update() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        // Authenticate.
        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();
        let user_id = auth_body["User"]["Id"].as_str().unwrap().to_string();

        // Discover the song GUID.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Audio")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let songs: Value = test::call_and_read_body_json(&app, req).await;
        let song_id = songs["Items"][0]["Id"].as_str().unwrap().to_string();

        // GET /UserItems/{id}/UserData on a fresh item → defaults.
        let req = test::TestRequest::get()
            .uri(&format!("/UserItems/{song_id}/UserData"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Played"], false);
        assert_eq!(ud["PlayCount"], 0);
        assert_eq!(ud["PlaybackPositionTicks"], 0);
        assert_eq!(ud["IsFavorite"], false);
        assert!(ud["Rating"].is_null());
        assert!(ud["Likes"].is_null());
        assert_eq!(ud["Key"], song_id);
        assert_eq!(ud["ItemId"], song_id);

        // POST /UserPlayedItems/{id} → Played=true, PlayCount=1, LastPlayedDate set.
        let req = test::TestRequest::post()
            .uri(&format!("/UserPlayedItems/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Played"], true);
        assert_eq!(ud["PlayCount"], 1);
        assert!(ud["LastPlayedDate"].as_str().unwrap().len() >= 19);

        // Same POST again — PlayCount increments.
        let req = test::TestRequest::post()
            .uri(&format!("/UserPlayedItems/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["PlayCount"], 2);

        // /Items/{id} detail now reflects the played state.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let item: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(item["UserData"]["Played"], true);
        assert_eq!(item["UserData"]["PlayCount"], 2);

        // POST /UserItems/{id}/Rating?likes=true → Likes=true.
        let req = test::TestRequest::post()
            .uri(&format!("/UserItems/{song_id}/Rating?likes=true"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Likes"], true);

        // Play state is preserved across Rating writes.
        assert_eq!(ud["PlayCount"], 2);
        assert_eq!(ud["Played"], true);

        // DELETE /UserItems/{id}/Rating → Likes cleared.
        let req = test::TestRequest::delete()
            .uri(&format!("/UserItems/{song_id}/Rating"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert!(ud["Likes"].is_null());

        // DELETE /UserPlayedItems/{id} → Played reset, but Likes stays cleared.
        let req = test::TestRequest::delete()
            .uri(&format!("/UserPlayedItems/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Played"], false);
        assert_eq!(ud["PlayCount"], 0);
        assert_eq!(ud["PlaybackPositionTicks"], 0);
        assert!(ud["LastPlayedDate"].is_null());

        // POST /UserItems/{id}/UserData with a full body — sets Rating,
        // Position, Played, and IsFavorite in a single call.
        let req = test::TestRequest::post()
            .uri(&format!("/UserItems/{song_id}/UserData"))
            .insert_header(("X-Emby-Token", token.clone()))
            .set_json(serde_json::json!({
                "Rating": 8.5,
                "PlaybackPositionTicks": 12_345_678_i64,
                "PlayCount": 5,
                "Played": true,
                "IsFavorite": true,
                "Likes": false,
                "LastPlayedDate": "2026-01-02T03:04:05.0000000",
            }))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Rating"], 8.5);
        assert_eq!(ud["PlaybackPositionTicks"], 12_345_678_i64);
        assert_eq!(ud["PlayCount"], 5);
        assert_eq!(ud["Played"], true);
        assert_eq!(ud["IsFavorite"], true);
        assert_eq!(ud["Likes"], false);
        assert_eq!(ud["LastPlayedDate"], "2026-01-02T03:04:05.0000000");

        // Persisted: fetching the item detail reflects everything above.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let item: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(item["UserData"]["Rating"], 8.5);
        assert_eq!(item["UserData"]["IsFavorite"], true);
        assert_eq!(item["UserData"]["PlaybackPositionTicks"], 12_345_678_i64);

        // Legacy per-user endpoints work the same way. Clear the played
        // flag via `/Users/{uid}/PlayedItems/{id}` (DELETE) and confirm.
        let req = test::TestRequest::delete()
            .uri(&format!("/Users/{user_id}/PlayedItems/{song_id}"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Played"], false);
        assert_eq!(ud["PlayCount"], 0);
        // Rating / IsFavorite / Likes survive the played-state reset.
        assert_eq!(ud["IsFavorite"], true);
        assert_eq!(ud["Rating"], 8.5);
        assert_eq!(ud["Likes"], false);

        // Legacy per-user rating endpoint — dislikes=false via query.
        let req = test::TestRequest::post()
            .uri(&format!(
                "/Users/{user_id}/Items/{song_id}/Rating?likes=false"
            ))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Likes"], false);

        // Legacy per-user GET UserData.
        let req = test::TestRequest::get()
            .uri(&format!("/Users/{user_id}/Items/{song_id}/UserData"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let ud: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(ud["Likes"], false);
        assert_eq!(ud["IsFavorite"], true);

        // Unknown GUID → 404 on every write path.
        let bogus = "00000000-0000-0000-0000-000000000000";
        for uri in [
            format!("/UserPlayedItems/{bogus}"),
            format!("/UserItems/{bogus}/Rating"),
            format!("/UserItems/{bogus}/UserData"),
        ] {
            let req = test::TestRequest::post()
                .uri(&uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .set_json(serde_json::json!({}))
                .to_request();
            assert_eq!(
                test::call_service(&app, req).await.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {uri}"
            );
        }
    }

    /// Exercises the InstantMix API (Jellyfin OpenAPI InstantMix tag) —
    /// artist, album, song, playlist, and genre seeds all return a
    /// `BaseItemDtoQueryResult` of Audio items honouring `?limit`.
    #[actix_web::test]
    async fn instant_mix_seeds_and_limit() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;

        // Insert a second artist + album + several extra songs so the mix
        // has enough material to exercise the three-tier fallback (same
        // artist → same genre → random library-wide).
        sqlx::query(
            "INSERT INTO artists (id, name, name_lower) VALUES ('ar-2','Another Artist','another artist')",
        )
        .execute(&state.pool).await.unwrap();
        sqlx::query(
            "INSERT INTO albums (id, title, artist, artist_id, year, cover_art)
             VALUES ('al-2','Other Album','Another Artist','ar-2',2021,NULL)",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        for (id, title, artist_id, album_id, genre) in [
            ("so-2", "Second Song", "ar-1", "al-1", Some("Rock")),
            ("so-3", "Third Song", "ar-1", "al-1", Some("Rock")),
            ("so-4", "Fourth Song", "ar-2", "al-2", Some("Rock")),
            ("so-5", "Fifth Song", "ar-2", "al-2", Some("Jazz")),
        ] {
            let path = dir.join(format!("{id}.mp3"));
            let mut f = std::fs::File::create(&path).unwrap();
            Write::write_all(&mut f, &[0u8; 4096]).unwrap();
            let genre_val = genre.map(|g| g.to_string());
            let artist_name = if artist_id == "ar-1" {
                "Test Artist"
            } else {
                "Another Artist"
            };
            let album_title = if album_id == "al-1" {
                "Test Album"
            } else {
                "Other Album"
            };
            sqlx::query(
                "INSERT INTO songs (id, path, title, artist, artist_id, album, album_id, genre,
                    track_number, disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art, mtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    1, 1, 2020, 60000, 192, 4096, 'mp3', 'audio/mpeg', NULL, 0)",
            )
            .bind(id)
            .bind(path.to_string_lossy().to_string())
            .bind(title)
            .bind(artist_name)
            .bind(artist_id)
            .bind(album_title)
            .bind(album_id)
            .bind(genre_val)
            .execute(&state.pool).await.unwrap();
        }

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();

        // Discover GUIDs for artist / album / song by iterating /Items.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicArtist")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let artists: Value = test::call_and_read_body_json(&app, req).await;
        let ar1 = artists["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["Name"] == "Test Artist")
            .unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_string();

        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicAlbum")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let albums: Value = test::call_and_read_body_json(&app, req).await;
        let al1 = albums["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["Name"] == "Test Album")
            .unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_string();

        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Audio")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let songs: Value = test::call_and_read_body_json(&app, req).await;
        let so1 = songs["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["Name"] == "Test Song")
            .unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_string();

        // Artist seed — every song by that artist plus filler up to limit.
        let req = test::TestRequest::get()
            .uri(&format!("/Artists/{ar1}/InstantMix?limit=5"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let mix: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(mix["TotalRecordCount"], 5);
        assert_eq!(mix["Items"][0]["MediaType"], "Audio");
        assert_eq!(mix["Items"][0]["Type"], "Audio");

        // Album seed.
        let req = test::TestRequest::get()
            .uri(&format!("/Albums/{al1}/InstantMix?limit=3"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let mix: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(mix["TotalRecordCount"], 3);

        // Song seed — first item MUST be the seed itself.
        let req = test::TestRequest::get()
            .uri(&format!("/Songs/{so1}/InstantMix?limit=4"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let mix: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(mix["TotalRecordCount"], 4);
        assert_eq!(mix["Items"][0]["Id"], so1);
        assert_eq!(mix["Items"][0]["Name"], "Test Song");
        // No duplicates.
        let ids: std::collections::HashSet<&str> = mix["Items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["Id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 4);

        // Same seed routed through /Items/{id}/InstantMix dispatches to the
        // song handler.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{so1}/InstantMix?limit=2"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let mix: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(mix["TotalRecordCount"], 2);
        assert_eq!(mix["Items"][0]["Id"], so1);

        // Genre seed by name.
        let req = test::TestRequest::get()
            .uri("/MusicGenres/Rock/InstantMix?limit=3")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let mix: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(mix["TotalRecordCount"], 3);
        // Every returned song must be Rock — the 3 rock songs are so-2..so-4.
        let names: std::collections::HashSet<&str> = mix["Items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["Name"].as_str().unwrap())
            .collect();
        for n in &names {
            assert!(
                ["Second Song", "Third Song", "Fourth Song"].contains(n),
                "unexpected non-Rock song in genre mix: {n}"
            );
        }

        // /MusicGenres/InstantMix?id=… — smolsonic returns an empty result
        // rather than 404 (no Genre BaseItems to resolve against).
        let req = test::TestRequest::get()
            .uri("/MusicGenres/InstantMix?id=00000000-0000-0000-0000-000000000000&limit=5")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let mix: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(mix["TotalRecordCount"], 0);

        // Playlist seed — mixes the playlist's songs first, then filler.
        let req = test::TestRequest::post()
            .uri("/Playlists")
            .insert_header(("X-Emby-Token", token.clone()))
            .set_json(serde_json::json!({
                "Name": "Mix Seed",
                "Ids": [so1.clone()],
                "MediaType": "Audio",
            }))
            .to_request();
        let created: Value = test::call_and_read_body_json(&app, req).await;
        let playlist_id = created["Id"].as_str().unwrap().to_string();
        let req = test::TestRequest::get()
            .uri(&format!("/Playlists/{playlist_id}/InstantMix?limit=5"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let mix: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(mix["TotalRecordCount"], 5);
        assert_eq!(mix["Items"][0]["Id"], so1);

        // Unknown GUID → 404 on each specific-kind endpoint.
        let bogus = "00000000-0000-0000-0000-000000000000";
        for uri in [
            format!("/Artists/{bogus}/InstantMix"),
            format!("/Albums/{bogus}/InstantMix"),
            format!("/Songs/{bogus}/InstantMix"),
            format!("/Playlists/{bogus}/InstantMix"),
            format!("/Items/{bogus}/InstantMix"),
        ] {
            let req = test::TestRequest::get()
                .uri(&uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            assert_eq!(
                test::call_service(&app, req).await.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {uri}"
            );
        }
    }

    /// End-to-end for the Lyric API modelled after the Jellyfin OpenAPI
    /// Lyric tag: GET on missing sidecar → 404; POST synced LRC round-trips;
    /// POST plain text → IsSynced=false; DELETE removes it; RemoteSearch
    /// returns [] and the download/preview stubs 404.
    #[actix_web::test]
    async fn lyrics_roundtrip_sidecar_and_remote_stubs() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        // Authenticate.
        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();

        // Discover the song's Jellyfin GUID.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Audio")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let songs: Value = test::call_and_read_body_json(&app, req).await;
        let song_id = songs["Items"][0]["Id"].as_str().unwrap().to_string();

        // Fresh song → no sidecar yet → GET returns 404.
        let req = test::TestRequest::get()
            .uri(&format!("/Audio/{song_id}/Lyrics"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NOT_FOUND
        );

        // POST a synced LRC — server persists it as `song.lrc` next to
        // `song.mp3` and echoes the parsed DTO.
        let lrc = "\
[ar:Test Artist]
[al:Test Album]
[ti:Test Song]
[length:01:00]
[00:12.34]First line
[00:16.78]Second line
";
        let req = test::TestRequest::post()
            .uri(&format!("/Audio/{song_id}/Lyrics?fileName=song.lrc"))
            .insert_header(("X-Emby-Token", token.clone()))
            .set_payload(lrc.as_bytes().to_vec())
            .to_request();
        let posted: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(posted["Metadata"]["Artist"], "Test Artist");
        assert_eq!(posted["Metadata"]["Album"], "Test Album");
        assert_eq!(posted["Metadata"]["Title"], "Test Song");
        assert_eq!(posted["Metadata"]["IsSynced"], true);
        assert_eq!(posted["Metadata"]["Length"], 600_000_000_i64);
        assert_eq!(posted["Lyrics"][0]["Text"], "First line");
        assert_eq!(posted["Lyrics"][0]["Start"], 123_400_000_i64);
        assert_eq!(posted["Lyrics"][1]["Text"], "Second line");
        assert_eq!(posted["Lyrics"][1]["Start"], 167_800_000_i64);

        // Sidecar file was written to disk.
        assert!(dir.join("song.lrc").exists());

        // Subsequent GET returns the same parsed body.
        let req = test::TestRequest::get()
            .uri(&format!("/Audio/{song_id}/Lyrics"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let got: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(got["Metadata"]["IsSynced"], true);
        assert_eq!(got["Lyrics"][0]["Start"], 123_400_000_i64);

        // POST plain text (unsynced) — IsSynced=false, Start=null.
        let plain = "Verse one\nVerse two\n";
        let req = test::TestRequest::post()
            .uri(&format!("/Audio/{song_id}/Lyrics?fileName=song.lrc"))
            .insert_header(("X-Emby-Token", token.clone()))
            .set_payload(plain.as_bytes().to_vec())
            .to_request();
        let posted: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(posted["Metadata"]["IsSynced"], false);
        assert_eq!(posted["Lyrics"][0]["Text"], "Verse one");
        assert!(posted["Lyrics"][0]["Start"].is_null());

        // RemoteSearch returns an empty array (no provider configured).
        let req = test::TestRequest::get()
            .uri(&format!("/Audio/{song_id}/RemoteSearch/Lyrics"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let hits: Value = test::call_and_read_body_json(&app, req).await;
        assert!(hits.as_array().unwrap().is_empty());

        // Download-remote and provider-preview are 404 (no provider).
        for uri in [
            format!("/Audio/{song_id}/RemoteSearch/Lyrics/some-lyric-id"),
            "/Providers/Lyrics/some-lyric-id".to_string(),
        ] {
            let method = if uri.contains("RemoteSearch") {
                actix_web::http::Method::POST
            } else {
                actix_web::http::Method::GET
            };
            let req = test::TestRequest::default()
                .method(method)
                .uri(&uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            assert_eq!(
                test::call_service(&app, req).await.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {uri}"
            );
        }

        // DELETE removes the sidecar; subsequent GET 404s.
        let req = test::TestRequest::delete()
            .uri(&format!("/Audio/{song_id}/Lyrics"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NO_CONTENT
        );
        assert!(!dir.join("song.lrc").exists());
        let req = test::TestRequest::get()
            .uri(&format!("/Audio/{song_id}/Lyrics"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NOT_FOUND
        );

        // DELETE again — no sidecar, still 204 (idempotent).
        let req = test::TestRequest::delete()
            .uri(&format!("/Audio/{song_id}/Lyrics"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NO_CONTENT
        );

        // Unknown GUID → 404 on every write path.
        let bogus = "00000000-0000-0000-0000-000000000000";
        for uri in [
            format!("/Audio/{bogus}/Lyrics"),
            format!("/Audio/{bogus}/RemoteSearch/Lyrics"),
        ] {
            let req = test::TestRequest::get()
                .uri(&uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            assert_eq!(
                test::call_service(&app, req).await.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {uri}"
            );
        }
    }

    /// Similar API — with neither Last.fm nor MusicBrainz configured, every
    /// endpoint returns an empty `ItemsResult`. The fixture doesn't set
    /// tokens so this exercises the plugin-disabled path. Movie/Trailer/
    /// Shows always return empty regardless.
    #[actix_web::test]
    async fn similar_returns_empty_when_no_plugin_tokens_configured() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, true).await;
        assert!(!state.similar.any_enabled(), "fixture must have no plugins");
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();

        // Discover the artist / album / video GUIDs.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicArtist")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let artists: Value = test::call_and_read_body_json(&app, req).await;
        let artist_id = artists["Items"][0]["Id"].as_str().unwrap().to_string();

        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicAlbum")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let albums: Value = test::call_and_read_body_json(&app, req).await;
        let album_id = albums["Items"][0]["Id"].as_str().unwrap().to_string();

        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=Movie")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let movies: Value = test::call_and_read_body_json(&app, req).await;
        let movie_id = movies["Items"][0]["Id"].as_str().unwrap().to_string();

        // Album / Artist similarity: plugins disabled → empty.
        for uri in [
            format!("/Albums/{album_id}/Similar"),
            format!("/Artists/{artist_id}/Similar"),
            format!("/Items/{album_id}/Similar"),
            format!("/Items/{artist_id}/Similar"),
        ] {
            let req = test::TestRequest::get()
                .uri(&uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            let body: Value = test::call_and_read_body_json(&app, req).await;
            assert_eq!(body["TotalRecordCount"], 0, "{uri}");
            assert!(body["Items"].as_array().unwrap().is_empty(), "{uri}");
        }

        // Movies / Trailers / Shows always empty (no music-similarity to
        // consult — provider status doesn't matter).
        let req = test::TestRequest::get()
            .uri(&format!("/Movies/{movie_id}/Similar"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body["TotalRecordCount"], 0);

        // Unknown GUID → 404 on album / artist / movie variants.
        let bogus = "00000000-0000-0000-0000-000000000000";
        for uri in [
            format!("/Albums/{bogus}/Similar"),
            format!("/Artists/{bogus}/Similar"),
            format!("/Movies/{bogus}/Similar"),
            format!("/Items/{bogus}/Similar"),
        ] {
            let req = test::TestRequest::get()
                .uri(&uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            assert_eq!(
                test::call_service(&app, req).await.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {uri}"
            );
        }
    }

    /// RemoteImage API — with neither Last.fm nor MusicBrainz configured,
    /// the search returns empty images + empty providers, Providers
    /// returns [], Download rejects with 400, and unknown GUIDs 404.
    #[actix_web::test]
    async fn remote_images_returns_empty_when_no_plugin_tokens_configured() {
        let dir = tempdir();
        let state = fixture_state(&dir, &dir, false).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/Users/AuthenticateByName")
            .insert_header((
                "X-Emby-Authorization",
                r#"MediaBrowser Client="t", Device="d", DeviceId="i", Version="v""#,
            ))
            .set_json(serde_json::json!({"Username":"alice","Pw":"secret"}))
            .to_request();
        let auth_body: Value = test::call_and_read_body_json(&app, req).await;
        let token = auth_body["AccessToken"].as_str().unwrap().to_string();

        // Discover the album GUID.
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicAlbum")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let albums: Value = test::call_and_read_body_json(&app, req).await;
        let album_id = albums["Items"][0]["Id"].as_str().unwrap().to_string();

        // GET /RemoteImages — no plugins → empty images + empty providers.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{album_id}/RemoteImages"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body["TotalRecordCount"], 0);
        assert!(body["Images"].as_array().unwrap().is_empty());
        assert!(body["Providers"].as_array().unwrap().is_empty());

        // GET /RemoteImages/Providers — no plugins → empty list.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{album_id}/RemoteImages/Providers"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert!(body.as_array().unwrap().is_empty());

        // POST Download without imageUrl → 400.
        let req = test::TestRequest::post()
            .uri(&format!(
                "/Items/{album_id}/RemoteImages/Download?type=Primary"
            ))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::BAD_REQUEST
        );

        // POST Download with non-Primary type → 400.
        let req = test::TestRequest::post()
            .uri(&format!(
                "/Items/{album_id}/RemoteImages/Download?type=Backdrop&imageUrl=http://x"
            ))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::BAD_REQUEST
        );

        // POST Download with plugins disabled but otherwise valid → 400.
        let req = test::TestRequest::post()
            .uri(&format!(
                "/Items/{album_id}/RemoteImages/Download?type=Primary&imageUrl=https://example/x.jpg"
            ))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::BAD_REQUEST
        );

        // Requesting a non-Primary type on search → empty images, no error.
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{album_id}/RemoteImages?type=Backdrop"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body["TotalRecordCount"], 0);

        // Unknown GUID → 404 across the trio.
        let bogus = "00000000-0000-0000-0000-000000000000";
        for uri in [
            format!("/Items/{bogus}/RemoteImages"),
            format!("/Items/{bogus}/RemoteImages/Providers"),
        ] {
            let req = test::TestRequest::get()
                .uri(&uri)
                .insert_header(("X-Emby-Token", token.clone()))
                .to_request();
            assert_eq!(
                test::call_service(&app, req).await.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {uri}"
            );
        }

        // Artist GUID → RemoteImages 404 (only album + song supported).
        let req = test::TestRequest::get()
            .uri("/Items?IncludeItemTypes=MusicArtist")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let artists: Value = test::call_and_read_body_json(&app, req).await;
        let artist_id = artists["Items"][0]["Id"].as_str().unwrap().to_string();
        let req = test::TestRequest::get()
            .uri(&format!("/Items/{artist_id}/RemoteImages"))
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        assert_eq!(
            test::call_service(&app, req).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
