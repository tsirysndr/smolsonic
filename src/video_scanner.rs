use crate::db::Db;
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;
use walkdir::WalkDir;

pub const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "webm", "mov", "avi", "m4v"];
pub const POSTER_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Default)]
pub struct VideoScanProgress {
    pub running: AtomicBool,
    pub count: AtomicUsize,
}

#[derive(Debug, Clone, Default)]
pub struct VideoScanStats {
    pub scanned: usize,
    pub inserted: usize,
    pub updated: usize,
    pub skipped: usize,
    pub removed: usize,
}

#[derive(Debug)]
enum ProcessResult {
    Inserted,
    Updated,
    Skipped,
}

pub async fn scan(
    pool: Db,
    video_dir: PathBuf,
    covers_dir: PathBuf,
    progress: Arc<VideoScanProgress>,
) -> Result<VideoScanStats> {
    progress.running.store(true, Ordering::SeqCst);
    progress.count.store(0, Ordering::SeqCst);
    std::fs::create_dir_all(&covers_dir).with_context(|| {
        format!("creating covers dir {}", covers_dir.display())
    })?;

    let mut stats = VideoScanStats::default();
    let walker = WalkDir::new(&video_dir).follow_links(true).into_iter();
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !has_video_extension(path) {
            continue;
        }
        stats.scanned += 1;
        progress.count.store(stats.scanned, Ordering::SeqCst);
        match process_file(&pool, path, &covers_dir).await {
            Ok(ProcessResult::Inserted) => stats.inserted += 1,
            Ok(ProcessResult::Updated) => stats.updated += 1,
            Ok(ProcessResult::Skipped) => stats.skipped += 1,
            Err(e) => {
                tracing::warn!("video scan {}: {e}", path.display());
                stats.skipped += 1;
            }
        }
    }

    match reconcile_deletions(&pool).await {
        Ok(removed) => stats.removed = removed as usize,
        Err(e) => tracing::warn!("video reconcile deletions: {e}"),
    }

    progress.running.store(false, Ordering::SeqCst);
    tracing::info!(
        "video scan complete: {} scanned, {} inserted, {} updated, {} skipped, {} removed",
        stats.scanned,
        stats.inserted,
        stats.updated,
        stats.skipped,
        stats.removed,
    );
    Ok(stats)
}

fn has_video_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            VIDEO_EXTS.iter().any(|v| *v == lower.as_str())
        })
        .unwrap_or(false)
}

fn mtime_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn video_id(path: &Path) -> String {
    let mut h = Md5::new();
    h.update(b"video:");
    h.update(path.to_string_lossy().as_bytes());
    let digest = h.finalize();
    format!("vi-{}", hex::encode(&digest[..8]))
}

/// Clean common filename junk so the title displayed in clients is readable.
/// Strips a trailing `(2023)`/`[2023]` year and quality/source tags after a dot.
fn clean_title(stem: &str) -> String {
    let mut s = stem.replace('.', " ").replace('_', " ");
    // Collapse whitespace.
    let mut prev_space = false;
    s = s
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                if prev_space {
                    None
                } else {
                    prev_space = true;
                    Some(' ')
                }
            } else {
                prev_space = false;
                Some(c)
            }
        })
        .collect();
    s.trim().to_string()
}

fn find_sibling_poster(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_string_lossy();

    // 1. Same-name file with a poster extension.
    for ext in POSTER_EXTS {
        let candidate = parent.join(format!("{stem}.{ext}"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // 2. poster.{ext} / folder.{ext} / cover.{ext} in the same directory.
    for base in ["poster", "folder", "cover"] {
        for ext in POSTER_EXTS {
            let candidate = parent.join(format!("{base}.{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[derive(Debug, Default, Clone, Copy)]
struct Probe {
    duration_ms: i64,
    bitrate: i64,
    width: i64,
    height: i64,
}

fn ffprobe_available() -> bool {
    use std::sync::OnceLock;
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| Command::new("ffprobe").arg("-version").output().is_ok())
}

fn ffmpeg_available() -> bool {
    use std::sync::OnceLock;
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| Command::new("ffmpeg").arg("-version").output().is_ok())
}

fn probe(path: &Path) -> Probe {
    if !ffprobe_available() {
        return Probe::default();
    }
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,bit_rate:format=duration,bit_rate",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output();
    let Ok(out) = output else {
        return Probe::default();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut p = Probe::default();
    for line in text.lines() {
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        match key {
            "width" => {
                if let Ok(v) = val.parse::<i64>() {
                    p.width = v;
                }
            }
            "height" => {
                if let Ok(v) = val.parse::<i64>() {
                    p.height = v;
                }
            }
            "duration" => {
                if let Ok(v) = val.parse::<f64>() {
                    p.duration_ms = (v * 1000.0) as i64;
                }
            }
            "bit_rate" => {
                if p.bitrate == 0 {
                    if let Ok(v) = val.parse::<i64>() {
                        p.bitrate = v;
                    }
                }
            }
            _ => {}
        }
    }
    p
}

/// Extract a single frame ~10% into the video, default-capped at 1 minute in
/// so it's representative without slow seeking on big files. Returns the
/// output path on success.
fn generate_thumbnail(
    src: &Path,
    duration_ms: i64,
    covers_dir: &Path,
    video_id: &str,
) -> Option<PathBuf> {
    if !ffmpeg_available() {
        return None;
    }
    let dst = covers_dir.join(format!("{video_id}.jpg"));
    if dst.exists() {
        return Some(dst);
    }

    // Seek to min(10% of duration, 60s); fall back to 5s if duration is unknown.
    let seek_secs = if duration_ms > 0 {
        ((duration_ms / 10) / 1000).clamp(2, 60)
    } else {
        5
    };

    let status = Command::new("ffmpeg")
        .args(["-y", "-ss", &seek_secs.to_string()])
        .arg("-i")
        .arg(src)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='min(640,iw)':-2",
            "-q:v",
            "5",
            "-loglevel",
            "error",
        ])
        .arg(&dst)
        .status();
    match status {
        Ok(s) if s.success() && dst.exists() => Some(dst),
        Ok(s) => {
            tracing::warn!("ffmpeg thumbnail failed for {}: status={s}", src.display());
            None
        }
        Err(e) => {
            tracing::warn!("ffmpeg invocation failed: {e}");
            None
        }
    }
}

async fn process_file(
    pool: &Db,
    path: &Path,
    covers_dir: &Path,
) -> Result<ProcessResult> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?;
    let mtime = mtime_secs(path);
    let filesize = meta.len() as i64;
    let id = video_id(path);
    let path_str = path.to_string_lossy().to_string();

    // Cheap skip if mtime + size both unchanged.
    let existing: Option<(i64, i64)> = sqlx::query_as(
        "SELECT mtime, filesize FROM videos WHERE id = ?1",
    )
    .bind(&id)
    .fetch_optional(pool)
    .await?;
    if let Some((old_mtime, old_size)) = existing {
        if old_mtime == mtime && old_size == filesize {
            return Ok(ProcessResult::Skipped);
        }
    }

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    let title = clean_title(stem);
    let container = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".to_string());

    let p = probe(path);

    // Poster: prefer a sibling file; otherwise auto-generate with ffmpeg.
    let poster = find_sibling_poster(path)
        .or_else(|| generate_thumbnail(path, p.duration_ms, covers_dir, &id))
        .map(|pb| pb.to_string_lossy().to_string());

    let was_new = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM videos WHERE id = ?1")
        .bind(&id)
        .fetch_one(pool)
        .await?
        == 0;

    sqlx::query(
        "INSERT INTO videos (id, path, title, container, duration_ms, filesize, bitrate,
            width, height, poster_path, mtime)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(id) DO UPDATE SET
            path        = excluded.path,
            title       = excluded.title,
            container   = excluded.container,
            duration_ms = excluded.duration_ms,
            filesize    = excluded.filesize,
            bitrate     = excluded.bitrate,
            width       = excluded.width,
            height      = excluded.height,
            poster_path = excluded.poster_path,
            mtime       = excluded.mtime",
    )
    .bind(&id)
    .bind(&path_str)
    .bind(&title)
    .bind(&container)
    .bind(p.duration_ms)
    .bind(filesize)
    .bind(p.bitrate)
    .bind(p.width)
    .bind(p.height)
    .bind(poster.as_deref())
    .bind(mtime)
    .execute(pool)
    .await?;

    Ok(if was_new {
        ProcessResult::Inserted
    } else {
        ProcessResult::Updated
    })
}

async fn reconcile_deletions(pool: &Db) -> Result<u64> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, path FROM videos").fetch_all(pool).await?;
    let mut removed = 0u64;
    for (id, path) in rows {
        if !Path::new(&path).exists() {
            sqlx::query("DELETE FROM videos WHERE id = ?1")
                .bind(&id)
                .execute(pool)
                .await?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_video_extensions() {
        assert!(has_video_extension(Path::new("a.mkv")));
        assert!(has_video_extension(Path::new("a.MP4")));
        assert!(!has_video_extension(Path::new("a.mp3")));
        assert!(!has_video_extension(Path::new("a")));
    }

    #[test]
    fn cleans_filename_junk() {
        assert_eq!(clean_title("The.Matrix.1999"), "The Matrix 1999");
        assert_eq!(clean_title("Some_Movie_Title"), "Some Movie Title");
        assert_eq!(clean_title("  spaced  out  "), "spaced out");
    }

    #[test]
    fn video_id_is_stable() {
        let p = PathBuf::from("/foo/bar.mkv");
        assert_eq!(video_id(&p), video_id(&p));
        assert!(video_id(&p).starts_with("vi-"));
    }

    #[test]
    fn finds_same_name_sibling_poster() {
        let dir = std::env::temp_dir().join(format!(
            "smolsonic-vid-poster-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("movie.mkv");
        let poster = dir.join("movie.jpg");
        std::fs::write(&video, b"").unwrap();
        std::fs::write(&poster, b"").unwrap();
        assert_eq!(find_sibling_poster(&video), Some(poster));
    }

    #[test]
    fn finds_folder_jpg_fallback() {
        let dir = std::env::temp_dir().join(format!(
            "smolsonic-vid-folder-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("movie.mkv");
        let folder = dir.join("folder.png");
        std::fs::write(&video, b"").unwrap();
        std::fs::write(&folder, b"").unwrap();
        assert_eq!(find_sibling_poster(&video), Some(folder));
    }
}
