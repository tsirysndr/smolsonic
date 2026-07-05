pub mod auth;
pub mod handlers;
pub mod repo;
pub mod response;

use crate::config::Config;
use crate::db::Db;
use crate::scanner::ScanProgress;
use crate::scrobble::ListenBrainzClient;
use crate::typesense::TypesenseClient;
use actix_cors::Cors;
use actix_web::{web, App, HttpServer};
use std::path::PathBuf;
use std::sync::Arc;

pub struct SubsonicState {
    pub pool: Db,
    pub username: Arc<String>,
    pub password: Arc<String>,
    pub music_dir: PathBuf,
    pub covers_dir: PathBuf,
    pub scan_progress: Arc<ScanProgress>,
    /// Optional Typesense client. When `Some`, `search3`/`search2` route
    /// through Typesense with fallback to FTS5 on error.
    pub typesense: Option<Arc<TypesenseClient>>,
    /// Optional ListenBrainz scrobble target. `None` → `/rest/scrobble` is
    /// still ack'd but no listens are submitted.
    pub scrobble: Option<Arc<ListenBrainzClient>>,
}

pub async fn start(
    cfg: Config,
    pool: Db,
    scan_progress: Arc<ScanProgress>,
    typesense: Option<Arc<TypesenseClient>>,
    scrobble: Option<Arc<ListenBrainzClient>>,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let state = web::Data::new(SubsonicState {
        pool,
        username: Arc::new(cfg.username.clone()),
        password: Arc::new(cfg.password.clone()),
        music_dir: cfg.music_dir.clone(),
        covers_dir: cfg.covers_dir.clone(),
        scan_progress,
        typesense,
        scrobble,
    });

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

/// All Subsonic REST routes. Extracted so tests can mount them on an
/// `App::configure(configure_routes)` against an in-memory state.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/", web::get().to(handlers::index))
        .route("/rest/ping{_:(\\.view)?}", web::get().to(handlers::ping))
        .route("/rest/ping{_:(\\.view)?}", web::post().to(handlers::ping))
        .route(
            "/rest/getUser{_:(\\.view)?}",
            web::get().to(handlers::get_user),
        )
        .route(
            "/rest/getUser{_:(\\.view)?}",
            web::post().to(handlers::get_user),
        )
        .route(
            "/rest/getMusicFolders{_:(\\.view)?}",
            web::get().to(handlers::get_music_folders),
        )
        .route(
            "/rest/getMusicFolders{_:(\\.view)?}",
            web::post().to(handlers::get_music_folders),
        )
        .route(
            "/rest/getArtists{_:(\\.view)?}",
            web::get().to(handlers::get_artists),
        )
        .route(
            "/rest/getArtists{_:(\\.view)?}",
            web::post().to(handlers::get_artists),
        )
        .route(
            "/rest/getArtist{_:(\\.view)?}",
            web::get().to(handlers::get_artist),
        )
        .route(
            "/rest/getArtist{_:(\\.view)?}",
            web::post().to(handlers::get_artist),
        )
        .route(
            "/rest/getAlbum{_:(\\.view)?}",
            web::get().to(handlers::get_album),
        )
        .route(
            "/rest/getAlbum{_:(\\.view)?}",
            web::post().to(handlers::get_album),
        )
        .route(
            "/rest/getSong{_:(\\.view)?}",
            web::get().to(handlers::get_song),
        )
        .route(
            "/rest/getSong{_:(\\.view)?}",
            web::post().to(handlers::get_song),
        )
        .route(
            "/rest/getAlbumList2{_:(\\.view)?}",
            web::get().to(handlers::get_album_list2),
        )
        .route(
            "/rest/getAlbumList2{_:(\\.view)?}",
            web::post().to(handlers::get_album_list2),
        )
        .route(
            "/rest/getCoverArt{_:(\\.view)?}",
            web::get().to(handlers::get_cover_art),
        )
        .route(
            "/rest/getCoverArt{_:(\\.view)?}",
            web::post().to(handlers::get_cover_art),
        )
        .route(
            "/rest/stream{_:(\\.view)?}",
            web::get().to(handlers::stream),
        )
        .route(
            "/rest/stream{_:(\\.view)?}",
            web::post().to(handlers::stream),
        )
        .route(
            "/rest/download{_:(\\.view)?}",
            web::get().to(handlers::stream),
        )
        .route(
            "/rest/download{_:(\\.view)?}",
            web::post().to(handlers::stream),
        )
        .route(
            "/rest/search3{_:(\\.view)?}",
            web::get().to(handlers::search3),
        )
        .route(
            "/rest/search3{_:(\\.view)?}",
            web::post().to(handlers::search3),
        )
        .route(
            "/rest/getScanStatus{_:(\\.view)?}",
            web::get().to(handlers::get_scan_status),
        )
        .route(
            "/rest/getScanStatus{_:(\\.view)?}",
            web::post().to(handlers::get_scan_status),
        )
        .route(
            "/rest/startScan{_:(\\.view)?}",
            web::get().to(handlers::start_scan),
        )
        .route(
            "/rest/startScan{_:(\\.view)?}",
            web::post().to(handlers::start_scan),
        )
        // Folder browsing
        .route(
            "/rest/getIndexes{_:(\\.view)?}",
            web::get().to(handlers::get_indexes),
        )
        .route(
            "/rest/getIndexes{_:(\\.view)?}",
            web::post().to(handlers::get_indexes),
        )
        .route(
            "/rest/getMusicDirectory{_:(\\.view)?}",
            web::get().to(handlers::get_music_directory),
        )
        .route(
            "/rest/getMusicDirectory{_:(\\.view)?}",
            web::post().to(handlers::get_music_directory),
        )
        // Genres
        .route(
            "/rest/getGenres{_:(\\.view)?}",
            web::get().to(handlers::get_genres),
        )
        .route(
            "/rest/getGenres{_:(\\.view)?}",
            web::post().to(handlers::get_genres),
        )
        .route(
            "/rest/getSongsByGenre{_:(\\.view)?}",
            web::get().to(handlers::get_songs_by_genre),
        )
        .route(
            "/rest/getSongsByGenre{_:(\\.view)?}",
            web::post().to(handlers::get_songs_by_genre),
        )
        // Random / starred
        .route(
            "/rest/getRandomSongs{_:(\\.view)?}",
            web::get().to(handlers::get_random_songs),
        )
        .route(
            "/rest/getRandomSongs{_:(\\.view)?}",
            web::post().to(handlers::get_random_songs),
        )
        .route(
            "/rest/getStarred{_:(\\.view)?}",
            web::get().to(handlers::get_starred),
        )
        .route(
            "/rest/getStarred{_:(\\.view)?}",
            web::post().to(handlers::get_starred),
        )
        .route(
            "/rest/getStarred2{_:(\\.view)?}",
            web::get().to(handlers::get_starred2),
        )
        .route(
            "/rest/getStarred2{_:(\\.view)?}",
            web::post().to(handlers::get_starred2),
        )
        .route("/rest/star{_:(\\.view)?}", web::get().to(handlers::star))
        .route("/rest/star{_:(\\.view)?}", web::post().to(handlers::star))
        .route(
            "/rest/unstar{_:(\\.view)?}",
            web::get().to(handlers::unstar),
        )
        .route(
            "/rest/unstar{_:(\\.view)?}",
            web::post().to(handlers::unstar),
        )
        // Scrobble + now-playing
        .route(
            "/rest/scrobble{_:(\\.view)?}",
            web::get().to(handlers::scrobble),
        )
        .route(
            "/rest/scrobble{_:(\\.view)?}",
            web::post().to(handlers::scrobble),
        )
        .route(
            "/rest/getNowPlaying{_:(\\.view)?}",
            web::get().to(handlers::get_now_playing),
        )
        .route(
            "/rest/getNowPlaying{_:(\\.view)?}",
            web::post().to(handlers::get_now_playing),
        )
        .route(
            "/rest/updateNowPlaying{_:(\\.view)?}",
            web::get().to(handlers::update_now_playing),
        )
        .route(
            "/rest/updateNowPlaying{_:(\\.view)?}",
            web::post().to(handlers::update_now_playing),
        )
        // Playlists
        .route(
            "/rest/getPlaylists{_:(\\.view)?}",
            web::get().to(handlers::get_playlists),
        )
        .route(
            "/rest/getPlaylists{_:(\\.view)?}",
            web::post().to(handlers::get_playlists),
        )
        .route(
            "/rest/getPlaylist{_:(\\.view)?}",
            web::get().to(handlers::get_playlist),
        )
        .route(
            "/rest/getPlaylist{_:(\\.view)?}",
            web::post().to(handlers::get_playlist),
        )
        .route(
            "/rest/createPlaylist{_:(\\.view)?}",
            web::get().to(handlers::create_playlist),
        )
        .route(
            "/rest/createPlaylist{_:(\\.view)?}",
            web::post().to(handlers::create_playlist),
        )
        .route(
            "/rest/updatePlaylist{_:(\\.view)?}",
            web::get().to(handlers::update_playlist),
        )
        .route(
            "/rest/updatePlaylist{_:(\\.view)?}",
            web::post().to(handlers::update_playlist),
        )
        .route(
            "/rest/deletePlaylist{_:(\\.view)?}",
            web::get().to(handlers::delete_playlist),
        )
        .route(
            "/rest/deletePlaylist{_:(\\.view)?}",
            web::post().to(handlers::delete_playlist),
        )
        // Artist / album info / similar / top / lyrics
        .route(
            "/rest/getArtistInfo{_:(\\.view)?}",
            web::get().to(handlers::get_artist_info),
        )
        .route(
            "/rest/getArtistInfo{_:(\\.view)?}",
            web::post().to(handlers::get_artist_info),
        )
        .route(
            "/rest/getArtistInfo2{_:(\\.view)?}",
            web::get().to(handlers::get_artist_info2),
        )
        .route(
            "/rest/getArtistInfo2{_:(\\.view)?}",
            web::post().to(handlers::get_artist_info2),
        )
        .route(
            "/rest/getAlbumInfo{_:(\\.view)?}",
            web::get().to(handlers::get_album_info),
        )
        .route(
            "/rest/getAlbumInfo{_:(\\.view)?}",
            web::post().to(handlers::get_album_info),
        )
        .route(
            "/rest/getAlbumInfo2{_:(\\.view)?}",
            web::get().to(handlers::get_album_info),
        )
        .route(
            "/rest/getAlbumInfo2{_:(\\.view)?}",
            web::post().to(handlers::get_album_info),
        )
        .route(
            "/rest/getSimilarSongs{_:(\\.view)?}",
            web::get().to(handlers::get_similar_songs),
        )
        .route(
            "/rest/getSimilarSongs{_:(\\.view)?}",
            web::post().to(handlers::get_similar_songs),
        )
        .route(
            "/rest/getSimilarSongs2{_:(\\.view)?}",
            web::get().to(handlers::get_similar_songs2),
        )
        .route(
            "/rest/getSimilarSongs2{_:(\\.view)?}",
            web::post().to(handlers::get_similar_songs2),
        )
        .route(
            "/rest/getTopSongs{_:(\\.view)?}",
            web::get().to(handlers::get_top_songs),
        )
        .route(
            "/rest/getTopSongs{_:(\\.view)?}",
            web::post().to(handlers::get_top_songs),
        )
        .route(
            "/rest/getLyrics{_:(\\.view)?}",
            web::get().to(handlers::get_lyrics),
        )
        .route(
            "/rest/getLyrics{_:(\\.view)?}",
            web::post().to(handlers::get_lyrics),
        )
        // Aliases for older API versions
        .route(
            "/rest/getAlbumList{_:(\\.view)?}",
            web::get().to(handlers::get_album_list),
        )
        .route(
            "/rest/getAlbumList{_:(\\.view)?}",
            web::post().to(handlers::get_album_list),
        )
        .route(
            "/rest/search2{_:(\\.view)?}",
            web::get().to(handlers::search2),
        )
        .route(
            "/rest/search2{_:(\\.view)?}",
            web::post().to(handlers::search2),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::scanner::ScanProgress;
    use actix_web::{http::StatusCode, test, App};
    use md5::{Digest, Md5};
    use serde_json::Value;
    use std::io::Write;
    use std::sync::atomic::Ordering;

    const USER: &str = "alice";
    const PASS: &str = "secret";

    fn token_qs(salt: &str) -> String {
        let mut h = Md5::new();
        h.update(PASS.as_bytes());
        h.update(salt.as_bytes());
        let t: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        format!("u={USER}&t={t}&s={salt}")
    }

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("smolsonic-sub-{tag}-{pid}-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    async fn fixture_state(
        music_dir: &std::path::Path,
        covers_dir: &std::path::Path,
    ) -> SubsonicState {
        let db_path = music_dir.join("test.db");
        let pool = db::init(&db_path).await.unwrap();

        // Two artists, two albums, two songs — enough to exercise list/index/search.
        let song_a = music_dir.join("song_a.mp3");
        // A small but recognizable payload so range checks have real data.
        std::fs::File::create(&song_a)
            .unwrap()
            .write_all(&vec![1u8; 4096])
            .unwrap();
        let song_b = music_dir.join("song_b.mp3");
        std::fs::File::create(&song_b)
            .unwrap()
            .write_all(&vec![2u8; 2048])
            .unwrap();

        sqlx::query("INSERT INTO artists (id, name, name_lower) VALUES ('ar-1','Aretha','aretha')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO artists (id, name, name_lower) VALUES ('ar-2','Beethoven','beethoven')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO albums (id, title, artist, artist_id, year, cover_art)
             VALUES ('al-1','Respect','Aretha','ar-1',1967,NULL),
                    ('al-2','Ninth','Beethoven','ar-2',1824,NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO songs (id, path, title, artist, artist_id, album, album_id, genre,
                track_number, disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art, mtime)
             VALUES ('so-1', ?1, 'Respect', 'Aretha', 'ar-1', 'Respect', 'al-1', 'Soul',
                1, 1, 1967, 60000, 192, 4096, 'mp3', 'audio/mpeg', NULL, 0)",
        )
        .bind(song_a.to_string_lossy().to_string())
        .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO songs (id, path, title, artist, artist_id, album, album_id, genre,
                track_number, disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art, mtime)
             VALUES ('so-2', ?1, 'Ode to Joy', 'Beethoven', 'ar-2', 'Ninth', 'al-2', 'Classical',
                4, 1, 1824, 30000, 128, 2048, 'mp3', 'audio/mpeg', NULL, 0)",
        )
        .bind(song_b.to_string_lossy().to_string())
        .execute(&pool).await.unwrap();

        SubsonicState {
            pool,
            username: Arc::new(USER.to_string()),
            password: Arc::new(PASS.to_string()),
            music_dir: music_dir.to_path_buf(),
            covers_dir: covers_dir.to_path_buf(),
            scan_progress: Arc::new(ScanProgress::default()),
            typesense: None,
            scrobble: None,
        }
    }

    fn build_app_state(state: SubsonicState) -> web::Data<SubsonicState> {
        web::Data::new(state)
    }

    async fn read_envelope(resp: actix_web::dev::ServiceResponse) -> Value {
        let body: Value = test::read_body_json(resp).await;
        body
    }

    #[actix_web::test]
    async fn ping_token_auth_succeeds() {
        let dir = tempdir("ping_ok");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/rest/ping?{}", token_qs("nacl")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_envelope(resp).await;
        assert_eq!(body["subsonic-response"]["status"], "ok");
        assert_eq!(body["subsonic-response"]["type"], "smolsonic");
    }

    #[actix_web::test]
    async fn ping_view_suffix_and_plaintext_password_also_work() {
        let dir = tempdir("ping_view");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        // plaintext p=
        let r = test::TestRequest::get()
            .uri(&format!("/rest/ping.view?u={USER}&p={PASS}"))
            .to_request();
        assert_eq!(test::call_service(&app, r).await.status(), StatusCode::OK);

        // enc:<hex>
        let enc = hex::encode(PASS.as_bytes());
        let r = test::TestRequest::get()
            .uri(&format!("/rest/ping.view?u={USER}&p=enc:{enc}"))
            .to_request();
        let resp = test::call_service(&app, r).await;
        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body["subsonic-response"]["status"], "ok");
    }

    #[actix_web::test]
    async fn ping_with_wrong_password_returns_error_envelope() {
        let dir = tempdir("ping_bad");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/rest/ping?u={USER}&p=wrong"))
            .to_request();
        // Subsonic returns 200 with a failed envelope (not HTTP 401).
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body["subsonic-response"]["status"], "failed");
        assert_eq!(body["subsonic-response"]["error"]["code"], 40);
    }

    #[actix_web::test]
    async fn get_music_folders_lists_one_folder() {
        let dir = tempdir("folders");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/rest/getMusicFolders?{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        let folders = &body["subsonic-response"]["musicFolders"]["musicFolder"];
        assert_eq!(folders.as_array().map(|a| a.len()), Some(1));
        assert_eq!(folders[0]["id"], 1);
        assert_eq!(folders[0]["name"], "Music");
    }

    #[actix_web::test]
    async fn browse_artists_albums_songs_chain() {
        let dir = tempdir("browse");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/rest/getArtists?{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        let index = &body["subsonic-response"]["artists"]["index"];
        let groups = index.as_array().unwrap();
        // Two artists, A and B, so two index groups.
        assert_eq!(groups.len(), 2);
        let names: Vec<&str> = groups.iter().map(|g| g["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));

        // getArtist returns its albums.
        let req = test::TestRequest::get()
            .uri(&format!("/rest/getArtist?id=ar-1&{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        let artist = &body["subsonic-response"]["artist"];
        assert_eq!(artist["name"], "Aretha");
        assert_eq!(artist["album"].as_array().unwrap().len(), 1);
        assert_eq!(artist["album"][0]["id"], "al-1");

        // getAlbum returns its songs with their durations and contentType.
        let req = test::TestRequest::get()
            .uri(&format!("/rest/getAlbum?id=al-1&{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        let album = &body["subsonic-response"]["album"];
        assert_eq!(album["name"], "Respect");
        assert_eq!(album["songCount"], 1);
        let song = &album["song"][0];
        assert_eq!(song["id"], "so-1");
        assert_eq!(song["contentType"], "audio/mpeg");
        assert_eq!(song["duration"], 60);

        // getSong by id.
        let req = test::TestRequest::get()
            .uri(&format!("/rest/getSong?id=so-2&{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body["subsonic-response"]["song"]["title"], "Ode to Joy");

        // Unknown id → error 70.
        let req = test::TestRequest::get()
            .uri(&format!("/rest/getAlbum?id=al-missing&{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body["subsonic-response"]["status"], "failed");
        assert_eq!(body["subsonic-response"]["error"]["code"], 70);
    }

    #[actix_web::test]
    async fn search3_finds_artist_album_and_song() {
        let dir = tempdir("search");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/rest/search3?query=respect&{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        let sr = &body["subsonic-response"]["searchResult3"];
        // "Respect" matches one album and one song; the artist Aretha isn't a name hit.
        assert!(sr["song"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["title"] == "Respect"));
        assert!(sr["album"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "Respect"));
    }

    #[actix_web::test]
    async fn stream_returns_full_body_and_supports_range_request() {
        let dir = tempdir("stream");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        // Full body — 4096 bytes of 0x01.
        let req = test::TestRequest::get()
            .uri(&format!("/rest/stream?id=so-1&{}", token_qs("s")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "audio/mpeg"
        );
        assert_eq!(
            resp.headers()
                .get("content-length")
                .unwrap()
                .to_str()
                .unwrap(),
            "4096"
        );
        let bytes = test::read_body(resp).await;
        assert_eq!(bytes.len(), 4096);
        assert!(bytes.iter().all(|b| *b == 1));

        // Range — first 100 bytes.
        let req = test::TestRequest::get()
            .uri(&format!("/rest/stream?id=so-1&{}", token_qs("s")))
            .insert_header(("Range", "bytes=0-99"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers()
                .get("content-length")
                .unwrap()
                .to_str()
                .unwrap(),
            "100"
        );
        let cr = resp
            .headers()
            .get("content-range")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cr, "bytes 0-99/4096");

        // Bad auth on stream returns failed envelope, not a partial.
        let req = test::TestRequest::get()
            .uri("/rest/stream?id=so-1&u=alice&p=nope")
            .to_request();
        let resp = test::call_service(&app, req).await;
        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body["subsonic-response"]["error"]["code"], 40);

        // Unknown song.
        let req = test::TestRequest::get()
            .uri(&format!("/rest/stream?id=so-nope&{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body["subsonic-response"]["error"]["code"], 70);
    }

    #[actix_web::test]
    async fn cover_art_returns_404_when_album_has_no_cover() {
        let dir = tempdir("cover");
        let state = fixture_state(&dir, &dir).await;
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/rest/getCoverArt?id=al-1&{}", token_qs("s")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn get_scan_status_reflects_progress_state() {
        let dir = tempdir("scan");
        let state = fixture_state(&dir, &dir).await;
        // Pre-set count to a known value to confirm it's wired through.
        state.scan_progress.count.store(42, Ordering::SeqCst);
        let app = test::init_service(
            App::new()
                .app_data(build_app_state(state))
                .configure(configure_routes),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/rest/getScanStatus?{}", token_qs("s")))
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        let st = &body["subsonic-response"]["scanStatus"];
        assert_eq!(st["scanning"], false);
        assert_eq!(st["count"], 42);
    }
}
