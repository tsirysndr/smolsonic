//! LRC parser + sibling-file resolver backing the Jellyfin Lyric API.
//!
//! smolsonic doesn't scan lyrics into the database — the source of truth is
//! a `.lrc` file next to the audio (`song.mp3` → `song.lrc`, case-insensitive
//! extension). This module handles reading, parsing, and writing that
//! sidecar. Embedded USLT tags aren't consulted; too many audio files in the
//! wild carry stale or misencoded ones, so we prefer the explicit sidecar.

use super::dto::{LyricDto, LyricLine, LyricMetadata};
use std::path::{Path, PathBuf};

/// Ticks per second — Jellyfin's canonical time unit throughout the DTOs.
const TICKS_PER_SEC: i64 = 10_000_000;

/// Return the sibling `.lrc` path for `audio` (`song.mp3` → `song.lrc`). We
/// don't check for existence; callers decide whether to read or write.
pub fn sidecar_path(audio: &Path) -> PathBuf {
    audio.with_extension("lrc")
}

/// Search for a `.lrc` next to `audio`, matching case-insensitively (some
/// filesystems preserve case; `.LRC` and `.Lrc` are both valid in the wild).
pub fn find_sidecar(audio: &Path) -> Option<PathBuf> {
    let target = sidecar_path(audio);
    if target.exists() {
        return Some(target);
    }
    let dir = audio.parent()?;
    let stem = audio.file_stem()?.to_string_lossy().to_ascii_lowercase();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.eq_ignore_ascii_case("lrc"))
            .unwrap_or(false)
            && p.file_stem()
                .map(|f| f.to_string_lossy().to_ascii_lowercase() == stem)
                .unwrap_or(false)
        {
            return Some(p);
        }
    }
    None
}

/// Parse an LRC file into a `LyricDto`. Recognises the standard header tags
/// (`[ar:]`, `[al:]`, `[ti:]`, `[au:]`, `[length:]`, `[by:]`, `[offset:]`,
/// `[re:]`, `[ve:]`) and timestamped lines (`[mm:ss.xx]…` — one or more
/// leading timestamps stack, so `[00:12.00][01:05.00]Chorus` yields two
/// timed entries with the same text).
///
/// Unrecognised header keys are ignored; lines without a timestamp become
/// unsynced entries with `start = None`. `IsSynced` is true iff at least one
/// line has a timestamp.
pub fn parse_lrc(source: &str) -> LyricDto {
    let mut meta = LyricMetadata::default();
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut any_synced = false;

    for raw in source.lines() {
        let line = raw.trim_start_matches('\u{FEFF}').trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        // Peel any number of leading `[…]` tags. Timestamps → timed entries;
        // known metadata keys → the meta struct; unknown keys → ignored.
        let mut cursor = line;
        let mut timestamps: Vec<i64> = Vec::new();
        loop {
            let Some(rest) = cursor.strip_prefix('[') else {
                break;
            };
            let Some(end) = rest.find(']') else {
                break;
            };
            let tag = &rest[..end];
            let after = &rest[end + 1..];

            if let Some(ticks) = parse_timestamp(tag) {
                timestamps.push(ticks);
                cursor = after;
                continue;
            }
            if let Some((key, value)) = split_tag(tag) {
                apply_meta(&mut meta, key, value);
                // Metadata tags typically live alone on a line; skip the
                // rest of the line so trailing text isn't captured as a
                // lyric.
                cursor = "";
                break;
            }
            // Unknown bracketed content — treat as regular text.
            break;
        }

        if timestamps.is_empty() {
            let text = cursor.trim();
            if !text.is_empty() {
                lines.push(LyricLine {
                    text: text.to_string(),
                    start: None,
                });
            }
        } else {
            any_synced = true;
            let text = cursor.trim().to_string();
            for start in timestamps {
                lines.push(LyricLine {
                    text: text.clone(),
                    start: Some(start),
                });
            }
        }
    }

    if any_synced {
        // Real players expect synced lines in order — LRC files usually are,
        // but stacked timestamps break the natural ordering.
        lines.sort_by_key(|l| l.start.unwrap_or(i64::MAX));
    }

    meta.is_synced = Some(any_synced);
    LyricDto {
        metadata: meta,
        lyrics: lines,
    }
}

fn parse_timestamp(tag: &str) -> Option<i64> {
    // `mm:ss.xx` or `mm:ss.xxx` — sub-second precision may be 2 or 3 digits.
    // `hh:mm:ss.xx` is also seen occasionally; parse either 2- or 3-piece.
    let mut parts: Vec<&str> = tag.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    // Split "ss.xx" on the last piece.
    let last = parts.pop()?;
    let (secs, frac) = match last.split_once('.') {
        Some((s, f)) => (s, f),
        None => (last, ""),
    };
    let secs: i64 = secs.parse().ok()?;
    let frac_ticks: i64 = if frac.is_empty() {
        0
    } else {
        // Normalise fractional digits to 100-ns ticks. e.g. "50" → 500 ms
        // → 5_000_000 ticks; "500" → 500 ms → 5_000_000 ticks.
        let scaled = match frac.len() {
            1 => frac.parse::<i64>().ok()? * 100,
            2 => frac.parse::<i64>().ok()? * 10,
            _ => {
                // Take at most 3 digits (ms precision).
                let s = &frac[..frac.len().min(3)];
                s.parse::<i64>().ok()?
            }
        };
        scaled * (TICKS_PER_SEC / 1000)
    };

    let mut minutes: i64 = 0;
    let mut hours: i64 = 0;
    if let Some(m) = parts.pop() {
        minutes = m.parse().ok()?;
    }
    if let Some(h) = parts.pop() {
        hours = h.parse().ok()?;
    }
    let total_secs = hours * 3600 + minutes * 60 + secs;
    Some(total_secs * TICKS_PER_SEC + frac_ticks)
}

fn split_tag(tag: &str) -> Option<(&str, &str)> {
    let (k, v) = tag.split_once(':')?;
    // Numeric key means we misread a timestamp — reject.
    if k.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((k.trim(), v.trim()))
}

fn apply_meta(meta: &mut LyricMetadata, key: &str, value: &str) {
    let v = value.to_string();
    match key.to_ascii_lowercase().as_str() {
        "ar" => meta.artist = Some(v),
        "al" => meta.album = Some(v),
        "ti" => meta.title = Some(v),
        "au" => meta.author = Some(v),
        "by" => meta.by = Some(v),
        "re" => meta.creator = Some(v),
        "ve" => meta.version = Some(v),
        "length" => meta.length = parse_length(value),
        "offset" => {
            meta.offset = value
                .parse::<i64>()
                .ok()
                .map(|ms| ms * (TICKS_PER_SEC / 1000))
        }
        _ => {}
    }
}

/// `[length:03:45]` → total ticks. Accepts `mm:ss` or `hh:mm:ss`.
fn parse_length(value: &str) -> Option<i64> {
    let parts: Vec<&str> = value.split(':').collect();
    let ticks = match parts.len() {
        2 => {
            let m: i64 = parts[0].parse().ok()?;
            let s: i64 = parts[1].parse().ok()?;
            (m * 60 + s) * TICKS_PER_SEC
        }
        3 => {
            let h: i64 = parts[0].parse().ok()?;
            let m: i64 = parts[1].parse().ok()?;
            let s: i64 = parts[2].parse().ok()?;
            (h * 3600 + m * 60 + s) * TICKS_PER_SEC
        }
        _ => return None,
    };
    Some(ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metadata_and_synced_lines() {
        let src = "\
[ar:Some Artist]
[al:Some Album]
[ti:Some Title]
[length:03:45]
[offset:250]
[00:12.34]First line
[00:16.78]Second line
";
        let out = parse_lrc(src);
        assert_eq!(out.metadata.artist.as_deref(), Some("Some Artist"));
        assert_eq!(out.metadata.album.as_deref(), Some("Some Album"));
        assert_eq!(out.metadata.title.as_deref(), Some("Some Title"));
        assert_eq!(out.metadata.length, Some(225 * TICKS_PER_SEC));
        assert_eq!(out.metadata.offset, Some(250 * (TICKS_PER_SEC / 1000)));
        assert_eq!(out.metadata.is_synced, Some(true));
        assert_eq!(out.lyrics.len(), 2);
        assert_eq!(out.lyrics[0].text, "First line");
        // 12.34s = 12_340 ms = 123_400_000 ticks
        assert_eq!(out.lyrics[0].start, Some(123_400_000));
        assert_eq!(out.lyrics[1].start, Some(167_800_000));
    }

    #[test]
    fn unsynced_lines_have_null_start_and_is_synced_false() {
        let src = "Verse one\nVerse two\n\nVerse three\n";
        let out = parse_lrc(src);
        assert_eq!(out.metadata.is_synced, Some(false));
        assert_eq!(out.lyrics.len(), 3);
        assert_eq!(out.lyrics[0].text, "Verse one");
        assert!(out.lyrics[0].start.is_none());
    }

    #[test]
    fn stacked_timestamps_produce_multiple_entries() {
        let src = "[00:10.00][00:20.00]Chorus\n";
        let out = parse_lrc(src);
        assert_eq!(out.lyrics.len(), 2);
        assert_eq!(out.lyrics[0].start, Some(100_000_000));
        assert_eq!(out.lyrics[1].start, Some(200_000_000));
    }

    #[test]
    fn sidecar_path_is_lowercase_extension() {
        let p = sidecar_path(std::path::Path::new("/music/track.mp3"));
        assert_eq!(p, std::path::Path::new("/music/track.lrc"));
    }
}
