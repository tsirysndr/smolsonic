use crate::db::Db;
use anyhow::{Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture};
use lofty::probe::Probe;
use lofty::tag::Accessor;
use md5::{Digest, Md5};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use walkdir::WalkDir;

pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "ogg", "flac", "m4a", "aac", "mp4", "alac", "wav", "wv", "mpc", "aiff", "aif", "opus",
    "ape", "wma",
];

#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    pub scanned: usize,
    pub inserted: usize,
    pub updated: usize,
    pub skipped: usize,
    pub removed: usize,
}

#[derive(Debug, Default)]
pub struct ScanProgress {
    pub running: std::sync::atomic::AtomicBool,
    pub count: AtomicUsize,
}

pub async fn scan(
    pool: Db,
    music_dir: PathBuf,
    covers_dir: PathBuf,
    progress: Arc<ScanProgress>,
) -> Result<ScanStats> {
    progress.running.store(true, Ordering::SeqCst);
    progress.count.store(0, Ordering::SeqCst);
    std::fs::create_dir_all(&covers_dir)
        .with_context(|| format!("creating covers dir {}", covers_dir.display()))?;

    let mut stats = ScanStats::default();
    let walker = WalkDir::new(&music_dir).follow_links(true).into_iter();
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !has_audio_extension(path) {
            continue;
        }
        stats.scanned += 1;
        progress.count.store(stats.scanned, Ordering::SeqCst);
        match process_file(&pool, path, &covers_dir).await {
            Ok(ProcessResult::Inserted) => stats.inserted += 1,
            Ok(ProcessResult::Updated) => stats.updated += 1,
            Ok(ProcessResult::Skipped) => stats.skipped += 1,
            Err(e) => {
                tracing::warn!("scan {}: {e}", path.display());
                stats.skipped += 1;
            }
        }
    }

    match reconcile_deletions(&pool).await {
        Ok(removed) => stats.removed = removed as usize,
        Err(e) => tracing::warn!("reconcile deletions: {e}"),
    }

    progress.running.store(false, Ordering::SeqCst);
    tracing::info!(
        "scan complete: {} scanned, {} inserted, {} updated, {} skipped, {} removed",
        stats.scanned,
        stats.inserted,
        stats.updated,
        stats.skipped,
        stats.removed
    );
    Ok(stats)
}

pub async fn reconcile_deletions(pool: &Db) -> Result<u64> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT path, album_id, artist_id FROM songs")
            .fetch_all(pool)
            .await?;

    let mut missing_paths = Vec::new();
    let mut album_ids = std::collections::HashSet::new();
    let mut artist_ids = std::collections::HashSet::new();
    for (path, album_id, artist_id) in rows {
        if !Path::new(&path).exists() {
            missing_paths.push(path);
            album_ids.insert(album_id);
            artist_ids.insert(artist_id);
        }
    }

    if missing_paths.is_empty() {
        return Ok(0);
    }

    let mut tx = pool.begin().await?;
    let mut deleted = 0u64;
    for path in &missing_paths {
        deleted += sqlx::query("DELETE FROM songs WHERE path = ?1")
            .bind(path)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;

    for album_id in &album_ids {
        gc_album(pool, album_id).await?;
    }
    for artist_id in &artist_ids {
        gc_artist(pool, artist_id).await?;
    }

    Ok(deleted)
}

pub async fn gc_album(pool: &Db, album_id: &str) -> Result<()> {
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM songs WHERE album_id = ?1")
        .bind(album_id)
        .fetch_one(pool)
        .await?;
    if remaining == 0 {
        sqlx::query("DELETE FROM albums WHERE id = ?1")
            .bind(album_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn gc_artist(pool: &Db, artist_id: &str) -> Result<()> {
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM songs WHERE artist_id = ?1")
        .bind(artist_id)
        .fetch_one(pool)
        .await?;
    if remaining == 0 {
        sqlx::query("DELETE FROM artists WHERE id = ?1")
            .bind(artist_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

enum ProcessResult {
    Inserted,
    Updated,
    Skipped,
}

pub fn has_audio_extension(path: &Path) -> bool {
    match path.extension().and_then(|s| s.to_str()) {
        Some(ext) => AUDIO_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)),
        None => false,
    }
}

pub async fn upsert_file(pool: &Db, path: &Path, covers_dir: &Path) -> Result<()> {
    process_file(pool, path, covers_dir).await.map(|_| ())
}

async fn process_file(pool: &Db, path: &Path, covers_dir: &Path) -> Result<ProcessResult> {
    let path_str = path.to_string_lossy().to_string();
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let filesize = meta.len() as i64;

    if let Some(existing_mtime) =
        sqlx::query_scalar::<_, i64>("SELECT mtime FROM songs WHERE path = ?1")
            .bind(&path_str)
            .fetch_optional(pool)
            .await?
    {
        if existing_mtime == mtime {
            return Ok(ProcessResult::Skipped);
        }
    }

    let extracted = tokio::task::spawn_blocking({
        let path = path.to_path_buf();
        let covers_dir = covers_dir.to_path_buf();
        move || extract_metadata(&path, filesize, &covers_dir)
    })
    .await??;

    let Extracted {
        title,
        artist,
        album_artist,
        album,
        genre,
        year,
        track,
        disc,
        duration_ms,
        bitrate,
        suffix,
        content_type,
        cover_filename,
    } = extracted;

    let display_artist = if !album_artist.is_empty() {
        album_artist
    } else {
        artist.clone()
    };

    let artist_id = artist_id_for(&display_artist);
    let album_id = album_id_for(&display_artist, &album, year);
    let song_id = song_id_for(&path_str);

    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"INSERT INTO artists (id, name, name_lower) VALUES (?1, ?2, ?3)
           ON CONFLICT(name_lower) DO UPDATE SET name = excluded.name"#,
    )
    .bind(&artist_id)
    .bind(&display_artist)
    .bind(display_artist.to_lowercase())
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO albums (id, title, artist, artist_id, year, cover_art)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)
           ON CONFLICT(id) DO UPDATE SET
             title = excluded.title,
             artist = excluded.artist,
             artist_id = excluded.artist_id,
             year = excluded.year,
             cover_art = COALESCE(excluded.cover_art, albums.cover_art)"#,
    )
    .bind(&album_id)
    .bind(&album)
    .bind(&display_artist)
    .bind(&artist_id)
    .bind(year)
    .bind(cover_filename.as_deref())
    .execute(&mut *tx)
    .await?;

    let existed = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM songs WHERE id = ?1")
        .bind(&song_id)
        .fetch_one(&mut *tx)
        .await?
        > 0;

    sqlx::query(
        r#"INSERT INTO songs
           (id, path, title, artist, artist_id, album, album_id, genre, track_number,
            disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art, mtime)
           VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
           ON CONFLICT(id) DO UPDATE SET
             path = excluded.path,
             title = excluded.title,
             artist = excluded.artist,
             artist_id = excluded.artist_id,
             album = excluded.album,
             album_id = excluded.album_id,
             genre = excluded.genre,
             track_number = excluded.track_number,
             disc_number = excluded.disc_number,
             year = excluded.year,
             duration_ms = excluded.duration_ms,
             bitrate = excluded.bitrate,
             filesize = excluded.filesize,
             suffix = excluded.suffix,
             content_type = excluded.content_type,
             cover_art = COALESCE(excluded.cover_art, songs.cover_art),
             mtime = excluded.mtime"#,
    )
    .bind(&song_id)
    .bind(&path_str)
    .bind(&title)
    .bind(&artist)
    .bind(&artist_id)
    .bind(&album)
    .bind(&album_id)
    .bind(genre.as_deref())
    .bind(track)
    .bind(disc)
    .bind(year_to_opt(year))
    .bind(duration_ms)
    .bind(bitrate)
    .bind(filesize)
    .bind(&suffix)
    .bind(&content_type)
    .bind(cover_filename.as_deref())
    .bind(mtime)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(if existed {
        ProcessResult::Updated
    } else {
        ProcessResult::Inserted
    })
}

struct Extracted {
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    genre: Option<String>,
    year: i64,
    track: Option<i64>,
    disc: Option<i64>,
    duration_ms: i64,
    bitrate: i64,
    suffix: String,
    content_type: String,
    cover_filename: Option<String>,
}

fn extract_metadata(path: &Path, filesize: i64, covers_dir: &Path) -> Result<Extracted> {
    let tagged = Probe::open(path)?.read()?;
    let props = tagged.properties();
    let duration_ms = props.duration().as_millis() as i64;
    let bitrate = props.audio_bitrate().unwrap_or(0) as i64;

    let primary_tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let title = primary_tag
        .and_then(|t| t.title().map(|s| s.into_owned()))
        .unwrap_or_else(|| file_stem(path));
    let artist = primary_tag
        .and_then(|t| t.artist().map(|s| s.into_owned()))
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let album_artist = primary_tag
        .and_then(|t| {
            t.get_string(&lofty::tag::ItemKey::AlbumArtist)
                .map(|s| s.to_string())
        })
        .unwrap_or_default();
    let album = primary_tag
        .and_then(|t| t.album().map(|s| s.into_owned()))
        .unwrap_or_else(|| "Unknown Album".to_string());
    let genre = primary_tag.and_then(|t| t.genre().map(|s| s.into_owned()));
    let year = primary_tag.and_then(|t| t.year()).unwrap_or(0) as i64;
    let track = primary_tag.and_then(|t| t.track()).map(|n| n as i64);
    let disc = primary_tag.and_then(|t| t.disk()).map(|n| n as i64);

    let suffix = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let content_type = mime_for_suffix(&suffix).to_string();

    let album_key = album_id_for(
        if album_artist.is_empty() {
            &artist
        } else {
            &album_artist
        },
        &album,
        year,
    );

    let cover_filename = if let Some(pic) = primary_tag.and_then(|t| t.pictures().first().cloned())
    {
        save_picture(covers_dir, &album_key, &pic).ok()
    } else {
        find_dir_cover(path, covers_dir, &album_key).ok().flatten()
    };

    let _ = filesize;
    Ok(Extracted {
        title,
        artist,
        album_artist,
        album,
        genre,
        year,
        track,
        disc,
        duration_ms,
        bitrate,
        suffix,
        content_type,
        cover_filename,
    })
}

fn save_picture(covers_dir: &Path, album_key: &str, pic: &Picture) -> Result<String> {
    let ext = match pic.mime_type() {
        Some(MimeType::Jpeg) => "jpg",
        Some(MimeType::Png) => "png",
        Some(MimeType::Gif) => "gif",
        Some(MimeType::Bmp) => "bmp",
        Some(MimeType::Tiff) => "tiff",
        _ => "jpg",
    };
    let filename = format!("{album_key}.{ext}");
    let full = covers_dir.join(&filename);
    if !full.exists() {
        std::fs::write(&full, pic.data())?;
    }
    Ok(filename)
}

fn find_dir_cover(audio_path: &Path, covers_dir: &Path, album_key: &str) -> Result<Option<String>> {
    let Some(dir) = audio_path.parent() else {
        return Ok(None);
    };
    const CANDIDATES: &[&str] = &[
        "cover.jpg",
        "cover.jpeg",
        "cover.png",
        "folder.jpg",
        "folder.jpeg",
        "folder.png",
        "front.jpg",
        "front.png",
        "album.jpg",
        "album.png",
    ];
    for cand in CANDIDATES {
        let p = dir.join(cand);
        if p.is_file() {
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("jpg")
                .to_lowercase();
            let filename = format!("{album_key}.{ext}");
            let dest = covers_dir.join(&filename);
            if !dest.exists() {
                std::fs::copy(&p, &dest)?;
            }
            return Ok(Some(filename));
        }
    }
    Ok(None)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown Title")
        .to_string()
}

fn year_to_opt(year: i64) -> Option<i64> {
    if year > 0 {
        Some(year)
    } else {
        None
    }
}

fn mime_for_suffix(suffix: &str) -> &'static str {
    match suffix {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "m4a" | "aac" | "mp4" | "alac" => "audio/mp4",
        "wav" => "audio/wav",
        "wma" => "audio/x-ms-wma",
        "opus" => "audio/opus",
        "aiff" | "aif" => "audio/aiff",
        "wv" => "audio/x-wavpack",
        "mpc" => "audio/x-musepack",
        "ape" => "audio/x-ape",
        _ => "application/octet-stream",
    }
}

fn short_md5(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(32);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s.truncate(16);
    s
}

pub fn artist_id_for(name: &str) -> String {
    format!("ar-{}", short_md5(&name.to_lowercase()))
}

pub fn album_id_for(artist: &str, album: &str, year: i64) -> String {
    format!(
        "al-{}",
        short_md5(&format!(
            "{}|{}|{}",
            artist.to_lowercase(),
            album.to_lowercase(),
            year
        ))
    )
}

pub fn song_id_for(path: &str) -> String {
    format!("so-{}", short_md5(path))
}
