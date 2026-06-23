mod cli;
mod config;
mod db;
mod models;
mod s3;
mod scanner;
mod server;
mod watcher;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;

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
    cli::print_banner(&cfg.host, cfg.port, &cfg.music_dir, s3_endpoint);

    let pool = db::init(&cfg.database_path).await?;
    let scan_progress = Arc::new(scanner::ScanProgress::default());

    if !args.no_scan {
        let pool_c = pool.clone();
        let music_dir = cfg.music_dir.clone();
        let covers_dir = cfg.covers_dir.clone();
        let progress = scan_progress.clone();
        tokio::spawn(async move {
            tracing::info!("starting library scan of {}", music_dir.display());
            if let Err(e) = scanner::scan(
                pool_c.clone(),
                music_dir.clone(),
                covers_dir.clone(),
                progress,
            )
            .await
            {
                tracing::error!("scan failed: {e}");
            }
            watcher::start(pool_c, music_dir, covers_dir);
        });
    } else {
        watcher::start(pool.clone(), cfg.music_dir.clone(), cfg.covers_dir.clone());
    }

    if let Some(s3_cfg) = cfg.s3.clone() {
        if s3_cfg.enabled {
            let music_dir = cfg.music_dir.clone();
            actix_web::rt::spawn(async move {
                if let Err(e) = s3::start(s3_cfg, music_dir).await {
                    tracing::error!("s3 server stopped: {e}");
                }
            });
        }
    }

    server::start(cfg, pool, scan_progress).await
}
