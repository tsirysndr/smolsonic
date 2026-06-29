pub mod auth;
pub mod discovery;
pub mod dto;
pub mod handlers;
pub mod mapping;

use crate::config::JellyfinConfig;
use crate::db::Db;
use crate::scanner::ScanProgress;
use crate::video_scanner::VideoScanProgress;
use actix_cors::Cors;
use actix_web::{web, App, HttpServer};
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
    music_scan_progress: Arc<ScanProgress>,
    video_scan_progress: Arc<VideoScanProgress>,
) -> anyhow::Result<()> {
    let server_id = auth::ensure_server_id(&pool).await?;
    let user_id = mapping::user_guid(&username);
    let addr = format!("{}:{}", cfg.host, cfg.port);

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
    })
    .bind(&addr)?
    .run()
    .await?;

    Ok(())
}

/// All Jellyfin routes. Extracted so tests can mount them on an
/// `App::configure(configure_routes)` against an in-memory state.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/", web::get().to(handlers::index))
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
                actix_web::HttpResponse::Ok().content_type("text/css").body("")
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
        .route("/Items/{id}", web::get().to(handlers::item_by_id))
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
        .route(
            "/ScheduledTasks",
            web::get().to(handlers::empty_array),
        )
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
        .route(
            "/Shows/{id}/Episodes",
            web::get().to(handlers::empty_items),
        )
        .route("/Shows/{id}/Seasons", web::get().to(handlers::empty_items))
        // Artists
        .route("/Artists", web::get().to(handlers::artists))
        .route("/Artists/AlbumArtists", web::get().to(handlers::artists))
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
        // Video stream
        .route("/Videos/{id}/stream", web::get().to(handlers::video_stream))
        .route("/Videos/{id}/stream", web::head().to(handlers::video_stream))
        .route(
            "/Videos/{id}/stream.{ext}",
            web::get().to(handlers::video_stream_ext),
        )
        .route(
            "/Videos/{id}/stream.{ext}",
            web::head().to(handlers::video_stream_ext),
        )
        // Sessions / scrobble
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
        .route(
            "/Users/{uid}/PlayedItems/{id}",
            web::post().to(handlers::user_played_item),
        )
        .route(
            "/Users/{uid}/PlayedItems/{id}",
            web::delete().to(handlers::user_played_item),
        )
        // Common probes that clients hit — answer empty so they stop retrying.
        .route(
            "/DisplayPreferences/{id}",
            web::get().to(handlers::displaypreferences),
        )
        .route("/Playlists", web::get().to(handlers::empty_items))
        .route(
            "/Users/{id}/Items/Suggestions",
            web::get().to(handlers::empty_items),
        )
        .route(
            "/Users/{id}/Views/{view}/Latest",
            web::get().to(handlers::empty_array),
        )
        .route("/Genres", web::get().to(handlers::empty_items))
        .route("/MusicGenres", web::get().to(handlers::empty_items));
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
        .execute(&pool).await.unwrap();
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
        }
    }

    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "smolsonic-jf-test-{}",
            std::process::id()
        ));
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

        let req = test::TestRequest::get().uri("/System/Info/Public").to_request();
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
        assert_eq!(test::call_service(&app, bad).await.status(), StatusCode::UNAUTHORIZED);

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
        let cl = resp.headers().get("content-length").unwrap().to_str().unwrap();
        assert_eq!(cl, "100");
        let cr = resp.headers().get("content-range").unwrap().to_str().unwrap();
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

        // Views: should include both Music AND Movies libraries.
        let req = test::TestRequest::get()
            .uri("/Users/me/Views")
            .insert_header(("X-Emby-Token", token.clone()))
            .to_request();
        let views: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(views["TotalRecordCount"], 2);
        let names: Vec<&str> = views["Items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["Name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"Music"));
        assert!(names.contains(&"Movies"));

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

        // HEAD on the video stream — direct play.
        let req = test::TestRequest::default()
            .method(actix_web::http::Method::HEAD)
            .uri(&format!("/Videos/{video_id}/stream?api_key={token}"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").unwrap().to_str().unwrap(),
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
            resp.headers().get("content-length").unwrap().to_str().unwrap(),
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
}
