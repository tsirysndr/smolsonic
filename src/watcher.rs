use crate::db::Db;
use crate::scanner::{gc_album, gc_artist, has_audio_extension, upsert_file};
use anyhow::Result;
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::new_debouncer;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_secs(2);

pub fn start(pool: Db, music_dir: PathBuf, covers_dir: PathBuf) {
    let rt = tokio::runtime::Handle::current();
    std::thread::Builder::new()
        .name("library-watcher".into())
        .spawn(move || {
            if let Err(e) = run(pool, music_dir, covers_dir, rt) {
                tracing::error!("watcher stopped: {e}");
            }
        })
        .expect("spawn watcher thread");
}

fn run(
    pool: Db,
    music_dir: PathBuf,
    covers_dir: PathBuf,
    rt: tokio::runtime::Handle,
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = new_debouncer(DEBOUNCE, None, tx)?;
    debouncer.watch(&music_dir, RecursiveMode::Recursive)?;
    tracing::info!("watching {} for changes", music_dir.display());

    for result in rx {
        match result {
            Ok(events) => {
                for ev in events {
                    if matches!(ev.event.kind, EventKind::Access(_) | EventKind::Other) {
                        continue;
                    }
                    for path in &ev.event.paths {
                        rt.block_on(apply_path(&pool, &covers_dir, path));
                    }
                }
            }
            Err(errors) => {
                for e in errors {
                    tracing::warn!("watcher error: {e}");
                }
            }
        }
    }
    drop(debouncer);
    Ok(())
}

async fn apply_path(pool: &Db, covers_dir: &Path, path: &Path) {
    if path.is_file() {
        if !has_audio_extension(path) {
            return;
        }
        match upsert_file(pool, path, covers_dir).await {
            Ok(_) => tracing::info!("watch upsert {}", path.display()),
            Err(e) => tracing::warn!("watch upsert {}: {e}", path.display()),
        }
        return;
    }

    if path.exists() {
        return;
    }

    let path_str = path.to_string_lossy().to_string();
    if has_audio_extension(path) {
        match delete_song(pool, &path_str).await {
            Ok(true) => tracing::info!("watch delete {path_str}"),
            Ok(false) => {}
            Err(e) => tracing::warn!("watch delete {path_str}: {e}"),
        }
        return;
    }

    match delete_songs_under(pool, &path_str).await {
        Ok(n) if n > 0 => tracing::info!("watch delete {n} songs under {path_str}"),
        Ok(_) => {}
        Err(e) => tracing::warn!("watch delete dir {path_str}: {e}"),
    }
}

async fn delete_song(pool: &Db, path: &str) -> Result<bool> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT album_id, artist_id FROM songs WHERE path = ?1",
    )
    .bind(path)
    .fetch_optional(pool)
    .await?;

    let Some((album_id, artist_id)) = row else {
        return Ok(false);
    };

    sqlx::query("DELETE FROM songs WHERE path = ?1")
        .bind(path)
        .execute(pool)
        .await?;

    gc_album(pool, &album_id).await?;
    gc_artist(pool, &artist_id).await?;
    Ok(true)
}

async fn delete_songs_under(pool: &Db, dir: &str) -> Result<u64> {
    let mut prefix = dir.trim_end_matches(std::path::MAIN_SEPARATOR).to_string();
    prefix.push(std::path::MAIN_SEPARATOR);
    let pattern = format!("{}%", escape_like(&prefix));

    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT album_id, artist_id FROM songs WHERE path LIKE ?1 ESCAPE '\\'")
            .bind(&pattern)
            .fetch_all(pool)
            .await?;

    if rows.is_empty() {
        return Ok(0);
    }

    let deleted = sqlx::query("DELETE FROM songs WHERE path LIKE ?1 ESCAPE '\\'")
        .bind(&pattern)
        .execute(pool)
        .await?
        .rows_affected();

    let mut seen_albums = std::collections::HashSet::new();
    let mut seen_artists = std::collections::HashSet::new();
    for (album_id, artist_id) in rows {
        if seen_albums.insert(album_id.clone()) {
            gc_album(pool, &album_id).await?;
        }
        if seen_artists.insert(artist_id.clone()) {
            gc_artist(pool, &artist_id).await?;
        }
    }
    Ok(deleted)
}

fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}
