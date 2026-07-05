mod cli;
mod config;
mod db;
mod jellyfin;
mod mdns;
mod models;
mod s3;
mod scanner;
mod scrobble;
mod server;
mod typesense;
mod video_scanner;
mod watcher;

use anyhow::Result;
use clap::Parser;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

#[actix_web::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = cli::Cli::parse();
    let cfg = config::Config::load(&args.config)?;

    let s3_endpoint = cfg
        .s3
        .as_ref()
        .filter(|s| s.enabled)
        .map(|s| (s.host.as_str(), s.port));
    let jellyfin_endpoint = cfg.jellyfin.as_ref().map(|j| (j.host.as_str(), j.port));
    cli::print_banner(
        &cfg.host,
        cfg.port,
        &cfg.music_dir,
        s3_endpoint,
        jellyfin_endpoint,
    );

    let pool = db::init(&cfg.database_path).await?;
    let scan_progress = Arc::new(scanner::ScanProgress::default());

    // Optional Typesense client — construct once, share via `Arc`. On startup
    // we call `bootstrap()` to create collections, then reindex from SQLite
    // if the songs collection is empty (fresh Typesense node self-heals).
    let typesense: Option<Arc<typesense::TypesenseClient>> =
        if let Some(ts_cfg) = cfg.typesense.as_ref() {
            let client = Arc::new(typesense::TypesenseClient::new(ts_cfg));
            match client.bootstrap().await {
                Ok(()) => {
                    let client_c = client.clone();
                    let pool_c = pool.clone();
                    tokio::spawn(async move {
                        match client_c.songs_empty().await {
                            Ok(true) => {
                                tracing::info!("typesense: songs collection empty, reindexing");
                                if let Err(e) = client_c.reindex_from_db(&pool_c).await {
                                    tracing::error!("typesense reindex failed: {e}");
                                }
                            }
                            Ok(false) => {
                                tracing::info!(
                                    "typesense: songs collection non-empty, skipping reindex"
                                );
                            }
                            Err(e) => {
                                tracing::error!("typesense describe songs: {e}");
                            }
                        }
                    });
                    Some(client)
                }
                Err(e) => {
                    tracing::error!("typesense: bootstrap failed ({e}) — falling back to FTS5");
                    None
                }
            }
        } else {
            None
        };

    // Optional ListenBrainz scrobble client. Same opt-in shape as the other
    // plugins — `None` when `[listenbrainz]` is absent from the config.
    let scrobble: Option<Arc<scrobble::ListenBrainzClient>> = cfg
        .listenbrainz
        .as_ref()
        .map(|lb| Arc::new(scrobble::ListenBrainzClient::new(lb)));
    tracing::info!(
        "listenbrainz scrobble: {}",
        if scrobble.is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );

    if !args.no_scan {
        let pool_c = pool.clone();
        let music_dir = cfg.music_dir.clone();
        let covers_dir = cfg.covers_dir.clone();
        let progress = scan_progress.clone();
        let ts_c = typesense.clone();
        tokio::spawn(async move {
            tracing::info!("starting library scan of {}", music_dir.display());
            if let Err(e) = scanner::scan(
                pool_c.clone(),
                music_dir.clone(),
                covers_dir.clone(),
                progress,
                ts_c.clone(),
            )
            .await
            {
                tracing::error!("scan failed: {e}");
            }
            watcher::start(pool_c, music_dir, covers_dir, ts_c);
        });
    } else {
        watcher::start(
            pool.clone(),
            cfg.music_dir.clone(),
            cfg.covers_dir.clone(),
            typesense.clone(),
        );
    }

    if cfg.scan_interval_secs > 0 {
        let pool_c = pool.clone();
        let music_dir = cfg.music_dir.clone();
        let covers_dir = cfg.covers_dir.clone();
        let progress = scan_progress.clone();
        let ts_c = typesense.clone();
        let interval = Duration::from_secs(cfg.scan_interval_secs);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if progress.running.load(Ordering::SeqCst) {
                    tracing::debug!("rescan: previous scan still running, skipping tick");
                    continue;
                }
                tracing::info!("periodic rescan of {}", music_dir.display());
                if let Err(e) = scanner::scan(
                    pool_c.clone(),
                    music_dir.clone(),
                    covers_dir.clone(),
                    progress.clone(),
                    ts_c.clone(),
                )
                .await
                {
                    tracing::error!("periodic rescan failed: {e}");
                }
            }
        });
    }

    if let Some(s3_cfg) = cfg.s3.clone() {
        if s3_cfg.enabled {
            let music_dir = cfg.music_dir.clone();
            let video_dir = cfg.video.as_ref().map(|v| v.video_dir.clone());
            actix_web::rt::spawn(async move {
                if let Err(e) = s3::start(s3_cfg, music_dir, video_dir).await {
                    tracing::error!("s3 server stopped: {e}");
                }
            });
        }
    }

    // Video library — enabled only when [video] block is present.
    // Always create the progress handle (even when [video] is absent) so we
    // can share a single placeholder Arc with the Jellyfin scan-trigger.
    let video_scan_progress = Arc::new(video_scanner::VideoScanProgress::default());
    if let Some(video_cfg) = cfg.video.clone() {
        let pool_c = pool.clone();
        let video_dir = video_cfg.video_dir.clone();
        let covers_dir = cfg.covers_dir.clone();
        let progress_c = video_scan_progress.clone();
        if !args.no_scan {
            tokio::spawn(async move {
                tracing::info!("starting video scan of {}", video_dir.display());
                if let Err(e) = video_scanner::scan(pool_c, video_dir, covers_dir, progress_c).await
                {
                    tracing::error!("video scan failed: {e}");
                }
            });
        }
        if video_cfg.scan_interval_secs > 0 {
            let pool_c = pool.clone();
            let video_dir = video_cfg.video_dir.clone();
            let covers_dir = cfg.covers_dir.clone();
            let interval = Duration::from_secs(video_cfg.scan_interval_secs);
            let progress = video_scan_progress.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    if progress.running.load(Ordering::SeqCst) {
                        tracing::debug!("video rescan: previous scan still running, skipping tick");
                        continue;
                    }
                    tracing::info!("periodic video rescan of {}", video_dir.display());
                    if let Err(e) = video_scanner::scan(
                        pool_c.clone(),
                        video_dir.clone(),
                        covers_dir.clone(),
                        progress.clone(),
                    )
                    .await
                    {
                        tracing::error!("periodic video rescan failed: {e}");
                    }
                }
            });
        }
    }

    // Jellyfin sidecar — enabled only when [jellyfin] block is present.
    let jellyfin_server_id = if let Some(jf_cfg) = cfg.jellyfin.clone() {
        match jellyfin::auth::ensure_server_id(&pool).await {
            Ok(id) => {
                let pool_c = pool.clone();
                let username = cfg.username.clone();
                let password = cfg.password.clone();
                let music_dir = cfg.music_dir.clone();
                let covers_dir = cfg.covers_dir.clone();
                let jf_cfg_c = jf_cfg.clone();
                let video_library_name = cfg.video.as_ref().map(|v| v.library_name.clone());
                let video_dir = cfg.video.as_ref().map(|v| v.video_dir.clone());
                let lastfm = cfg.lastfm.clone();
                let musicbrainz = cfg.musicbrainz.clone();
                let music_progress = scan_progress.clone();
                let video_progress = video_scan_progress.clone();
                let ts_c = typesense.clone();
                let lb_c = scrobble.clone();
                actix_web::rt::spawn(async move {
                    if let Err(e) = jellyfin::start(
                        jf_cfg_c,
                        pool_c,
                        username,
                        password,
                        music_dir,
                        covers_dir,
                        video_library_name,
                        video_dir,
                        lastfm,
                        musicbrainz,
                        music_progress,
                        video_progress,
                        ts_c,
                        lb_c,
                    )
                    .await
                    {
                        tracing::error!("jellyfin server stopped: {e}");
                    }
                });

                let server_name = jf_cfg.server_name.clone();
                let server_id_c = id.clone();
                let http_port = jf_cfg.port;
                actix_web::rt::spawn(async move {
                    if let Err(e) =
                        jellyfin::discovery::run(server_name, server_id_c, http_port).await
                    {
                        tracing::error!("jellyfin discovery stopped: {e}");
                    }
                });

                Some(id)
            }
            Err(e) => {
                tracing::error!("jellyfin: failed to initialize server id: {e}");
                None
            }
        }
    } else {
        None
    };

    let _mdns_handle = if cfg.mdns.enabled {
        let s3_endpoint = cfg
            .s3
            .as_ref()
            .filter(|s| s.enabled)
            .map(|s| (s.host.clone(), s.port));
        let jellyfin_mdns = cfg
            .jellyfin
            .as_ref()
            .zip(jellyfin_server_id.as_ref())
            .map(|(j, id)| (j.host.clone(), j.port, id.clone()));
        match mdns::start(
            &cfg.mdns.instance_name,
            cfg.port,
            s3_endpoint,
            jellyfin_mdns,
        ) {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!("mdns: disabled — {e}");
                None
            }
        }
    } else {
        None
    };

    server::start(cfg, pool, scan_progress, typesense, scrobble).await
}
