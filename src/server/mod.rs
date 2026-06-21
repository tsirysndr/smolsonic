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
    })
    .bind(&addr)?
    .run()
    .await?;

    Ok(())
}
