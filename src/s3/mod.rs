pub mod handlers;
pub mod sigv4;

use crate::config::S3Config;
use actix_web::{web, App, HttpServer};
use std::path::PathBuf;
use std::sync::Arc;

pub const BUCKET: &str = "music";
pub const REGION: &str = "us-east-1";
pub const SERVICE: &str = "s3";

const MAX_UPLOAD: usize = 8 * 1024 * 1024 * 1024;

pub struct S3State {
    pub music_dir: PathBuf,
    pub access_key: Arc<String>,
    pub secret_key: Arc<String>,
}

pub async fn start(cfg: S3Config, music_dir: PathBuf) -> anyhow::Result<()> {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let state = web::Data::new(S3State {
        music_dir,
        access_key: Arc::new(cfg.access_key),
        secret_key: Arc::new(cfg.secret_key),
    });

    tracing::info!("starting S3 API on {addr} (bucket={BUCKET}, region={REGION})");

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(web::PayloadConfig::new(MAX_UPLOAD))
            .route("/", web::get().to(handlers::list_buckets))
            .route("/{bucket}", web::get().to(handlers::list_objects))
            .route("/{bucket}/", web::get().to(handlers::list_objects))
            .route("/{bucket}/{key:.*}", web::get().to(handlers::get_object))
            .route("/{bucket}/{key:.*}", web::head().to(handlers::head_object))
            .route("/{bucket}/{key:.*}", web::put().to(handlers::put_object))
            .route(
                "/{bucket}/{key:.*}",
                web::delete().to(handlers::delete_object),
            )
    })
    .bind(&addr)?
    .run()
    .await?;
    Ok(())
}
