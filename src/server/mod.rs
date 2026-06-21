pub mod auth;
pub mod handlers;
pub mod repo;
pub mod response;

use crate::config::Config;
use crate::db::Db;
use crate::scanner::ScanProgress;
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
}

pub async fn start(
    cfg: Config,
    pool: Db,
    scan_progress: Arc<ScanProgress>,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let state = web::Data::new(SubsonicState {
        pool,
        username: Arc::new(cfg.username.clone()),
        password: Arc::new(cfg.password.clone()),
        music_dir: cfg.music_dir.clone(),
        covers_dir: cfg.covers_dir.clone(),
        scan_progress,
    });

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .wrap(Cors::permissive())
            .route("/", web::get().to(handlers::index))
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
            )
    })
    .bind(&addr)?
    .run()
    .await?;

    Ok(())
}
