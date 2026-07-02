use crate::db::Db;
use crate::models::{Album, Artist, Playlist, Song, Video};
use anyhow::Result;

// ── Videos ──────────────────────────────────────────────────────────────────

pub async fn all_videos(pool: &Db, limit: i64, offset: i64) -> Result<Vec<Video>> {
    let rows = sqlx::query_as::<_, Video>(
        "SELECT id, path, title, container, duration_ms, filesize, bitrate,
                width, height, poster_path
         FROM videos ORDER BY title COLLATE NOCASE LIMIT ?1 OFFSET ?2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn find_video(pool: &Db, id: &str) -> Result<Option<Video>> {
    let row = sqlx::query_as::<_, Video>(
        "SELECT id, path, title, container, duration_ms, filesize, bitrate,
                width, height, poster_path
         FROM videos WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn count_videos(pool: &Db) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM videos")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// Build the WHERE clause for a `NameStartsWith` / `NameStartsWithOrGreater`
/// / `NameLessThan` filter against `column`, plus the values that get bound
/// for each `?n` placeholder. Returns an empty string when no filter is set.
/// Used by every list endpoint that exposes Jellyfin's alpha-jump rail.
fn name_filter_sql(
    column: &str,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
) -> (String, Vec<String>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    if let Some(prefix) = name_starts_with.filter(|s| !s.is_empty()) {
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        binds.push(format!("{escaped}%"));
        clauses.push(format!(
            "{column} LIKE ?{} ESCAPE '\\' COLLATE NOCASE",
            binds.len()
        ));
    }
    if let Some(b) = name_starts_with_or_greater.filter(|s| !s.is_empty()) {
        let first = b.chars().next().unwrap().to_string();
        binds.push(first);
        clauses.push(format!(
            "UPPER(SUBSTR({column}, 1, 1)) >= UPPER(?{})",
            binds.len()
        ));
    }
    if let Some(b) = name_less_than.filter(|s| !s.is_empty()) {
        let first = b.chars().next().unwrap().to_string();
        binds.push(first);
        clauses.push(format!(
            "UPPER(SUBSTR({column}, 1, 1)) < UPPER(?{})",
            binds.len()
        ));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    (where_sql, binds)
}

/// Distinct uppercase first letters of a name-style column, with non-alpha
/// rows grouped under "#". Drives Jellyfin's alpha-jump rail.
async fn name_prefixes(pool: &Db, table: &str, column: &str) -> Result<Vec<String>> {
    let sql =
        format!("SELECT DISTINCT UPPER(SUBSTR({column}, 1, 1)) FROM {table} WHERE {column} != ''");
    let rows: Vec<(String,)> = sqlx::query_as(&sql).fetch_all(pool).await?;
    let mut letters: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut has_other = false;
    for (c,) in rows {
        match c.chars().next() {
            Some(ch) if ch.is_ascii_alphabetic() => {
                letters.insert(c);
            }
            _ => {
                has_other = true;
            }
        }
    }
    let mut out: Vec<String> = letters.into_iter().collect();
    if has_other {
        out.insert(0, "#".to_string());
    }
    Ok(out)
}

pub async fn videos_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Video>> {
    let (where_sql, binds) = name_filter_sql(
        "title",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let limit_idx = binds.len() + 1;
    let offset_idx = binds.len() + 2;
    let sql = format!(
        "SELECT id, path, title, container, duration_ms, filesize, bitrate,
                width, height, poster_path
         FROM videos {where_sql} ORDER BY title COLLATE NOCASE
         LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );
    let mut q = sqlx::query_as::<_, Video>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    Ok(rows)
}

pub async fn count_videos_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
) -> Result<i64> {
    let (where_sql, binds) = name_filter_sql(
        "title",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let sql = format!("SELECT COUNT(*) FROM videos {where_sql}");
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let n = q.fetch_one(pool).await?;
    Ok(n)
}

pub async fn video_name_prefixes(pool: &Db) -> Result<Vec<String>> {
    name_prefixes(pool, "videos", "title").await
}

pub async fn artist_name_prefixes(pool: &Db) -> Result<Vec<String>> {
    name_prefixes(pool, "artists", "name").await
}

pub async fn album_name_prefixes(pool: &Db) -> Result<Vec<String>> {
    name_prefixes(pool, "albums", "title").await
}

pub async fn song_name_prefixes(pool: &Db) -> Result<Vec<String>> {
    name_prefixes(pool, "songs", "title").await
}

pub async fn artists_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Artist>> {
    let (where_sql, binds) = name_filter_sql(
        "name",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let limit_idx = binds.len() + 1;
    let offset_idx = binds.len() + 2;
    let sql = format!(
        "SELECT id, name FROM artists {where_sql}
         ORDER BY name COLLATE NOCASE LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );
    let mut q = sqlx::query_as::<_, Artist>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    Ok(rows)
}

pub async fn count_artists_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
) -> Result<i64> {
    let (where_sql, binds) = name_filter_sql(
        "name",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let sql = format!("SELECT COUNT(*) FROM artists {where_sql}");
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    Ok(q.fetch_one(pool).await?)
}

pub async fn albums_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Album>> {
    let (where_sql, binds) = name_filter_sql(
        "title",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let limit_idx = binds.len() + 1;
    let offset_idx = binds.len() + 2;
    let sql = format!(
        "SELECT id, title, artist, artist_id, year, cover_art FROM albums {where_sql}
         ORDER BY title COLLATE NOCASE LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );
    let mut q = sqlx::query_as::<_, Album>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    Ok(rows)
}

pub async fn count_albums_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
) -> Result<i64> {
    let (where_sql, binds) = name_filter_sql(
        "title",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let sql = format!("SELECT COUNT(*) FROM albums {where_sql}");
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    Ok(q.fetch_one(pool).await?)
}

pub async fn songs_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Song>> {
    let (where_sql, binds) = name_filter_sql(
        "title",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let limit_idx = binds.len() + 1;
    let offset_idx = binds.len() + 2;
    let sql = format!(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs {where_sql} ORDER BY title COLLATE NOCASE
         LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );
    let mut q = sqlx::query_as::<_, Song>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    Ok(rows)
}

pub async fn count_songs_filtered(
    pool: &Db,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    name_less_than: Option<&str>,
) -> Result<i64> {
    let (where_sql, binds) = name_filter_sql(
        "title",
        name_starts_with,
        name_starts_with_or_greater,
        name_less_than,
    );
    let sql = format!("SELECT COUNT(*) FROM songs {where_sql}");
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    Ok(q.fetch_one(pool).await?)
}

pub async fn search_videos(pool: &Db, term: &str, limit: i64, offset: i64) -> Result<Vec<Video>> {
    // Simple LIKE search — videos don't have FTS like songs do.
    let pattern = format!("%{}%", term.replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query_as::<_, Video>(
        "SELECT id, path, title, container, duration_ms, filesize, bitrate,
                width, height, poster_path
         FROM videos WHERE title LIKE ?1 ESCAPE '\\'
         ORDER BY title COLLATE NOCASE LIMIT ?2 OFFSET ?3",
    )
    .bind(pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn all_artists(pool: &Db) -> Result<Vec<Artist>> {
    let rows =
        sqlx::query_as::<_, Artist>("SELECT id, name FROM artists ORDER BY name COLLATE NOCASE")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

pub async fn find_artist(pool: &Db, id: &str) -> Result<Option<Artist>> {
    let row = sqlx::query_as::<_, Artist>("SELECT id, name FROM artists WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn album_counts_by_artist(pool: &Db) -> Result<std::collections::HashMap<String, i64>> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT artist_id, COUNT(*) FROM albums GROUP BY artist_id")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

pub async fn albums_by_artist(pool: &Db, artist_id: &str) -> Result<Vec<Album>> {
    let rows = sqlx::query_as::<_, Album>(
        "SELECT id, title, artist, artist_id, year, cover_art FROM albums
         WHERE artist_id = ?1 ORDER BY year, title COLLATE NOCASE",
    )
    .bind(artist_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn find_album(pool: &Db, id: &str) -> Result<Option<Album>> {
    let row = sqlx::query_as::<_, Album>(
        "SELECT id, title, artist, artist_id, year, cover_art FROM albums WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Set `albums.cover_art` to `filename` (a bare filename inside covers_dir,
/// not a path). Used by `POST /Items/{id}/RemoteImages/Download` after we
/// persist the downloaded bytes.
pub async fn set_album_cover_art(pool: &Db, album_id: &str, filename: &str) -> Result<()> {
    sqlx::query("UPDATE albums SET cover_art = ?1 WHERE id = ?2")
        .bind(filename)
        .bind(album_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn all_songs_paginated(pool: &Db, limit: i64, offset: i64) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs ORDER BY title COLLATE NOCASE LIMIT ?1 OFFSET ?2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn count_songs(pool: &Db) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM songs")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

pub async fn count_artists(pool: &Db) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artists")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

pub async fn count_albums(pool: &Db) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM albums")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

pub async fn songs_by_artist(
    pool: &Db,
    artist_id: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs WHERE artist_id = ?1
         ORDER BY album COLLATE NOCASE, disc_number, track_number
         LIMIT ?2 OFFSET ?3",
    )
    .bind(artist_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn songs_by_album(pool: &Db, album_id: &str) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs WHERE album_id = ?1
         ORDER BY disc_number, track_number, title COLLATE NOCASE",
    )
    .bind(album_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn find_song(pool: &Db, id: &str) -> Result<Option<Song>> {
    let row = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Build an FTS5 MATCH expression: each whitespace-separated token becomes a
/// quoted prefix term ANDed together. e.g. `tay swift` → `"tay"* "swift"*`.
/// Returns None when the input has no usable tokens.
fn fts5_match(term: &str) -> Option<String> {
    let parts: Vec<String> = term
        .split_whitespace()
        .filter_map(|w| {
            let cleaned: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || matches!(*c, '_' | '-'))
                .collect();
            if cleaned.is_empty() {
                None
            } else {
                Some(format!("\"{}\"*", cleaned))
            }
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

pub async fn search_artists(pool: &Db, term: &str, limit: i64, offset: i64) -> Result<Vec<Artist>> {
    let Some(q) = fts5_match(term) else {
        let rows = sqlx::query_as::<_, Artist>(
            "SELECT id, name FROM artists ORDER BY name COLLATE NOCASE LIMIT ?1 OFFSET ?2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        return Ok(rows);
    };
    let rows = sqlx::query_as::<_, Artist>(
        "SELECT a.id, a.name
         FROM artists_fts f INNER JOIN artists a ON a.id = f.id
         WHERE f.artists_fts MATCH ?1
         ORDER BY f.rank
         LIMIT ?2 OFFSET ?3",
    )
    .bind(q)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn search_albums(pool: &Db, term: &str, limit: i64, offset: i64) -> Result<Vec<Album>> {
    let Some(q) = fts5_match(term) else {
        let rows = sqlx::query_as::<_, Album>(
            "SELECT id, title, artist, artist_id, year, cover_art FROM albums
             ORDER BY title COLLATE NOCASE LIMIT ?1 OFFSET ?2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        return Ok(rows);
    };
    let rows = sqlx::query_as::<_, Album>(
        "SELECT a.id, a.title, a.artist, a.artist_id, a.year, a.cover_art
         FROM albums_fts f INNER JOIN albums a ON a.id = f.id
         WHERE f.albums_fts MATCH ?1
         ORDER BY f.rank
         LIMIT ?2 OFFSET ?3",
    )
    .bind(q)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn search_songs(pool: &Db, term: &str, limit: i64, offset: i64) -> Result<Vec<Song>> {
    let Some(q) = fts5_match(term) else {
        let rows = sqlx::query_as::<_, Song>(
            "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                    disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
             FROM songs ORDER BY title COLLATE NOCASE LIMIT ?1 OFFSET ?2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        return Ok(rows);
    };
    let rows = sqlx::query_as::<_, Song>(
        "SELECT s.id, s.path, s.title, s.artist, s.artist_id, s.album, s.album_id, s.genre,
                s.track_number, s.disc_number, s.year, s.duration_ms, s.bitrate, s.filesize,
                s.suffix, s.content_type, s.cover_art
         FROM songs_fts f INNER JOIN songs s ON s.id = f.id
         WHERE f.songs_fts MATCH ?1
         ORDER BY f.rank
         LIMIT ?2 OFFSET ?3",
    )
    .bind(q)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn songs_for_album_duration(pool: &Db, album_id: &str) -> Result<i64> {
    let total: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(duration_ms), 0) FROM songs WHERE album_id = ?1")
            .bind(album_id)
            .fetch_one(pool)
            .await?;
    Ok(total / 1000)
}

pub async fn song_count_for_album(pool: &Db, album_id: &str) -> Result<i64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM songs WHERE album_id = ?1")
        .bind(album_id)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

pub async fn random_songs(
    pool: &Db,
    size: i64,
    from_year: Option<i64>,
    to_year: Option<i64>,
    genre: Option<&str>,
) -> Result<Vec<Song>> {
    let mut q = String::from(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs WHERE 1=1",
    );
    if from_year.is_some() {
        q.push_str(" AND year >= ?");
    }
    if to_year.is_some() {
        q.push_str(" AND year <= ?");
    }
    if genre.is_some() {
        q.push_str(" AND genre = ? COLLATE NOCASE");
    }
    q.push_str(" ORDER BY RANDOM() LIMIT ?");

    let mut query = sqlx::query_as::<_, Song>(&q);
    if let Some(y) = from_year {
        query = query.bind(y);
    }
    if let Some(y) = to_year {
        query = query.bind(y);
    }
    if let Some(g) = genre {
        query = query.bind(g);
    }
    query = query.bind(size);
    Ok(query.fetch_all(pool).await?)
}

pub async fn distinct_genres(pool: &Db) -> Result<Vec<(String, i64, i64)>> {
    // (name, song_count, album_count)
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT genre, COUNT(*) AS song_count,
                (SELECT COUNT(DISTINCT s2.album_id) FROM songs s2 WHERE s2.genre = songs.genre) AS album_count
         FROM songs WHERE genre IS NOT NULL AND genre <> ''
         GROUP BY genre ORDER BY genre COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Per-genre stats for the `/Genres/{name}` and `/MusicGenres/{name}`
/// endpoints. Returns `None` when no songs carry that genre. Case-insensitive
/// match to be forgiving of client input.
pub async fn find_genre_stats(pool: &Db, name: &str) -> Result<Option<(String, i64, i64)>> {
    let row: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT genre, COUNT(*) AS song_count,
                (SELECT COUNT(DISTINCT s2.album_id) FROM songs s2 WHERE s2.genre = songs.genre COLLATE NOCASE) AS album_count
         FROM songs WHERE genre = ?1 COLLATE NOCASE
         GROUP BY genre LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Distinct non-zero years across songs + albums for the legacy
/// `QueryFiltersLegacy.Years` field. Sorted ascending.
pub async fn distinct_years(pool: &Db) -> Result<Vec<i32>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT year FROM (
            SELECT year FROM songs WHERE year IS NOT NULL AND year > 0
            UNION
            SELECT year FROM albums WHERE year > 0
         ) ORDER BY year",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(y,)| y as i32).collect())
}

/// Per-year stats for the `/Years/{year}` endpoint: (song_count, album_count).
/// Returns None when the year has no rows.
pub async fn find_year_stats(pool: &Db, year: i32) -> Result<Option<(i64, i64)>> {
    let song_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM songs WHERE year = ?1")
        .bind(year as i64)
        .fetch_one(pool)
        .await?;
    let album_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM albums WHERE year = ?1")
        .bind(year as i64)
        .fetch_one(pool)
        .await?;
    if song_count == 0 && album_count == 0 {
        return Ok(None);
    }
    Ok(Some((song_count, album_count)))
}

pub async fn songs_by_year(pool: &Db, year: i32, limit: i64, offset: i64) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs WHERE year = ?1
         ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, track_number
         LIMIT ?2 OFFSET ?3",
    )
    .bind(year as i64)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn albums_by_year(pool: &Db, year: i32) -> Result<Vec<Album>> {
    let rows = sqlx::query_as::<_, Album>(
        "SELECT id, title, artist, artist_id, year, cover_art FROM albums
         WHERE year = ?1 ORDER BY title COLLATE NOCASE",
    )
    .bind(year as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Random `size` songs whose native artist_id matches. Backs the artist /
/// album / song seeds in `/…/InstantMix` — each seed narrows to the artist,
/// then falls back to genre/random filler in the handler.
pub async fn random_songs_by_artist(pool: &Db, artist_id: &str, size: i64) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs WHERE artist_id = ?1
         ORDER BY RANDOM() LIMIT ?2",
    )
    .bind(artist_id)
    .bind(size)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn songs_by_genre(pool: &Db, genre: &str, count: i64, offset: i64) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs WHERE genre = ?1 COLLATE NOCASE
         ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, track_number
         LIMIT ?2 OFFSET ?3",
    )
    .bind(genre)
    .bind(count)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ── Starred ───────────────────────────────────────────────────────────────────

pub async fn star(pool: &Db, id: &str, when: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO starred (id, starred_at) VALUES (?1, ?2)
         ON CONFLICT(id) DO UPDATE SET starred_at = excluded.starred_at",
    )
    .bind(id)
    .bind(when)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unstar(pool: &Db, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM starred WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn starred_songs(pool: &Db) -> Result<Vec<(Song, String)>> {
    let pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT st.id, st.starred_at FROM starred st
         INNER JOIN songs s ON s.id = st.id
         ORDER BY st.starred_at DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(pairs.len());
    for (id, when) in pairs {
        if let Some(s) = find_song(pool, &id).await? {
            out.push((s, when));
        }
    }
    Ok(out)
}

pub async fn starred_albums(pool: &Db) -> Result<Vec<(Album, String)>> {
    let pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT st.id, st.starred_at FROM starred st
         INNER JOIN albums a ON a.id = st.id
         ORDER BY st.starred_at DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(pairs.len());
    for (id, when) in pairs {
        if let Some(a) = find_album(pool, &id).await? {
            out.push((a, when));
        }
    }
    Ok(out)
}

pub async fn starred_artists(pool: &Db) -> Result<Vec<(Artist, String)>> {
    let pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT st.id, st.starred_at FROM starred st
         INNER JOIN artists ar ON ar.id = st.id
         ORDER BY st.starred_at DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(pairs.len());
    for (id, when) in pairs {
        if let Some(a) = find_artist(pool, &id).await? {
            out.push((a, when));
        }
    }
    Ok(out)
}

pub async fn starred_videos(pool: &Db) -> Result<Vec<(Video, String)>> {
    let pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT st.id, st.starred_at FROM starred st
         INNER JOIN videos v ON v.id = st.id
         ORDER BY st.starred_at DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(pairs.len());
    for (id, when) in pairs {
        if let Some(v) = find_video(pool, &id).await? {
            out.push((v, when));
        }
    }
    Ok(out)
}

pub async fn starred_playlists(pool: &Db) -> Result<Vec<(Playlist, String)>> {
    let pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT st.id, st.starred_at FROM starred st
         INNER JOIN playlists p ON p.id = st.id
         ORDER BY st.starred_at DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(pairs.len());
    for (id, when) in pairs {
        if let Some(p) = find_playlist(pool, &id).await? {
            out.push((p, when));
        }
    }
    Ok(out)
}

/// True iff `native_id` is present in the `starred` table. Cheap lookup —
/// safe to call from every `*_to_dto` helper so `UserItemDataDto.IsFavorite`
/// reflects real state.
pub async fn is_starred(pool: &Db, native_id: &str) -> Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM starred WHERE id = ?1 LIMIT 1")
        .bind(native_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

// ── UserItemData ──────────────────────────────────────────────────────────────

/// Snapshot of every non-favorite field in Jellyfin's `UserItemDataDto`.
/// Rows missing from `user_item_data` map to `UserItemData::default()` —
/// clients see "fresh" state for anything they haven't touched yet.
#[derive(Debug, Clone, Default)]
pub struct UserItemData {
    pub played: bool,
    pub play_count: i32,
    pub playback_position_ticks: i64,
    pub last_played_date: Option<String>,
    pub rating: Option<f64>,
    pub likes: Option<bool>,
}

/// Songs whose playback is in progress — `playback_position_ticks > 0` and
/// not (yet) marked played. Ordered by `last_played_date DESC` so the most
/// recently seeked item lands first. Powers the home-screen "Resume" rail.
pub async fn resume_songs(pool: &Db, limit: i64) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT s.id, s.path, s.title, s.artist, s.artist_id, s.album, s.album_id, s.genre,
                s.track_number, s.disc_number, s.year, s.duration_ms, s.bitrate, s.filesize,
                s.suffix, s.content_type, s.cover_art
         FROM songs s
         INNER JOIN user_item_data u ON u.id = s.id
         WHERE u.playback_position_ticks > 0 AND u.played = 0
         ORDER BY COALESCE(u.last_played_date, '') DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Same for videos. Movies are the natural "Resume" candidate on the video
/// library home screen.
pub async fn resume_videos(pool: &Db, limit: i64) -> Result<Vec<Video>> {
    let rows = sqlx::query_as::<_, Video>(
        "SELECT v.id, v.path, v.title, v.container, v.duration_ms, v.filesize, v.bitrate,
                v.width, v.height, v.poster_path
         FROM videos v
         INNER JOIN user_item_data u ON u.id = v.id
         WHERE u.playback_position_ticks > 0 AND u.played = 0
         ORDER BY COALESCE(u.last_played_date, '') DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_user_item_data(pool: &Db, native_id: &str) -> Result<UserItemData> {
    let row: Option<(i64, i64, i64, Option<String>, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT played, play_count, playback_position_ticks, last_played_date, rating, likes
         FROM user_item_data WHERE id = ?1",
    )
    .bind(native_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        None => UserItemData::default(),
        Some((played, play_count, ticks, last, rating, likes)) => UserItemData {
            played: played != 0,
            play_count: play_count as i32,
            playback_position_ticks: ticks,
            last_played_date: last,
            rating,
            likes: likes.map(|v| v != 0),
        },
    })
}

/// Increment `PlayCount` by 1, set `Played=true`, and stamp `LastPlayedDate`.
/// Mirrors Jellyfin's `MarkPlayedItem` handler — resets the resume position
/// to 0 because the item has just finished playing.
pub async fn mark_played(pool: &Db, native_id: &str, when: &str) -> Result<UserItemData> {
    sqlx::query(
        "INSERT INTO user_item_data (id, played, play_count, playback_position_ticks, last_played_date)
         VALUES (?1, 1, 1, 0, ?2)
         ON CONFLICT(id) DO UPDATE SET
            played = 1,
            play_count = play_count + 1,
            playback_position_ticks = 0,
            last_played_date = excluded.last_played_date",
    )
    .bind(native_id)
    .bind(when)
    .execute(pool)
    .await?;
    get_user_item_data(pool, native_id).await
}

/// Reset `Played=false`, `PlayCount=0`, `PlaybackPositionTicks=0`, and clear
/// `LastPlayedDate`. Rating / likes are preserved.
pub async fn mark_unplayed(pool: &Db, native_id: &str) -> Result<UserItemData> {
    sqlx::query(
        "INSERT INTO user_item_data (id, played, play_count, playback_position_ticks, last_played_date)
         VALUES (?1, 0, 0, 0, NULL)
         ON CONFLICT(id) DO UPDATE SET
            played = 0,
            play_count = 0,
            playback_position_ticks = 0,
            last_played_date = NULL",
    )
    .bind(native_id)
    .execute(pool)
    .await?;
    get_user_item_data(pool, native_id).await
}

/// `POST /UserItems/{itemId}/Rating?likes=…` sets thumbs-up/down; DELETE on
/// the same path clears it. Pass `None` to clear.
pub async fn set_likes(pool: &Db, native_id: &str, likes: Option<bool>) -> Result<UserItemData> {
    let value: Option<i64> = likes.map(|v| if v { 1 } else { 0 });
    sqlx::query(
        "INSERT INTO user_item_data (id, likes) VALUES (?1, ?2)
         ON CONFLICT(id) DO UPDATE SET likes = excluded.likes",
    )
    .bind(native_id)
    .bind(value)
    .execute(pool)
    .await?;
    get_user_item_data(pool, native_id).await
}

/// Partial update body from `POST /UserItems/{itemId}/UserData`. The outer
/// `Option` distinguishes "field omitted" from "field present". For nullable
/// fields (last_played_date, rating, likes) the inner `Option` distinguishes
/// "present with value" from "present but null → clear".
#[derive(Debug, Default, Clone)]
pub struct UserItemDataUpdate {
    pub played: Option<bool>,
    pub play_count: Option<i32>,
    pub playback_position_ticks: Option<i64>,
    pub last_played_date: Option<Option<String>>,
    pub rating: Option<Option<f64>>,
    pub likes: Option<Option<bool>>,
}

pub async fn update_user_item_data(
    pool: &Db,
    native_id: &str,
    update: UserItemDataUpdate,
) -> Result<UserItemData> {
    let mut current = get_user_item_data(pool, native_id).await?;
    if let Some(v) = update.played {
        current.played = v;
    }
    if let Some(v) = update.play_count {
        current.play_count = v;
    }
    if let Some(v) = update.playback_position_ticks {
        current.playback_position_ticks = v;
    }
    if let Some(v) = update.last_played_date {
        current.last_played_date = v;
    }
    if let Some(v) = update.rating {
        current.rating = v;
    }
    if let Some(v) = update.likes {
        current.likes = v;
    }
    sqlx::query(
        "INSERT INTO user_item_data
            (id, played, play_count, playback_position_ticks, last_played_date, rating, likes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            played = excluded.played,
            play_count = excluded.play_count,
            playback_position_ticks = excluded.playback_position_ticks,
            last_played_date = excluded.last_played_date,
            rating = excluded.rating,
            likes = excluded.likes",
    )
    .bind(native_id)
    .bind(if current.played { 1_i64 } else { 0_i64 })
    .bind(current.play_count as i64)
    .bind(current.playback_position_ticks)
    .bind(current.last_played_date.clone())
    .bind(current.rating)
    .bind(current.likes.map(|v| if v { 1_i64 } else { 0_i64 }))
    .execute(pool)
    .await?;
    Ok(current)
}

// ── Playlists ─────────────────────────────────────────────────────────────────

pub async fn all_playlists(pool: &Db) -> Result<Vec<Playlist>> {
    let rows = sqlx::query_as::<_, Playlist>(
        "SELECT id, name, comment, public, created_at, updated_at FROM playlists
         ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn find_playlist(pool: &Db, id: &str) -> Result<Option<Playlist>> {
    let row = sqlx::query_as::<_, Playlist>(
        "SELECT id, name, comment, public, created_at, updated_at FROM playlists WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn create_playlist(pool: &Db, id: &str, name: &str, now: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO playlists (id, name, comment, public, created_at, updated_at)
         VALUES (?1, ?2, NULL, 1, ?3, ?3)",
    )
    .bind(id)
    .bind(name)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn rename_playlist(pool: &Db, id: &str, name: &str, now: &str) -> Result<()> {
    sqlx::query("UPDATE playlists SET name = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(name)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_playlist_comment(pool: &Db, id: &str, comment: &str, now: &str) -> Result<()> {
    sqlx::query("UPDATE playlists SET comment = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(comment)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_playlist(pool: &Db, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM playlists WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM playlist_songs WHERE playlist_id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn playlist_song_ids(pool: &Db, playlist_id: &str) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT song_id FROM playlist_songs WHERE playlist_id = ?1 ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(s,)| s).collect())
}

pub async fn playlist_songs(pool: &Db, playlist_id: &str) -> Result<Vec<Song>> {
    let rows = sqlx::query_as::<_, Song>(
        "SELECT s.id, s.path, s.title, s.artist, s.artist_id, s.album, s.album_id, s.genre,
                s.track_number, s.disc_number, s.year, s.duration_ms, s.bitrate, s.filesize,
                s.suffix, s.content_type, s.cover_art
         FROM playlist_songs ps INNER JOIN songs s ON s.id = ps.song_id
         WHERE ps.playlist_id = ?1
         ORDER BY ps.position",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn append_playlist_songs(
    pool: &Db,
    playlist_id: &str,
    song_ids: &[String],
) -> Result<()> {
    if song_ids.is_empty() {
        return Ok(());
    }
    let next_pos: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_songs WHERE playlist_id = ?1",
    )
    .bind(playlist_id)
    .fetch_one(pool)
    .await?;
    let mut tx = pool.begin().await?;
    for (i, sid) in song_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_songs (playlist_id, position, song_id) VALUES (?1, ?2, ?3)",
        )
        .bind(playlist_id)
        .bind(next_pos + i as i64)
        .bind(sid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn replace_playlist_songs(
    pool: &Db,
    playlist_id: &str,
    song_ids: &[String],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM playlist_songs WHERE playlist_id = ?1")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await?;
    for (i, sid) in song_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_songs (playlist_id, position, song_id) VALUES (?1, ?2, ?3)",
        )
        .bind(playlist_id)
        .bind(i as i64)
        .bind(sid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn albums_paginated(
    pool: &Db,
    list_type: &str,
    size: i64,
    offset: i64,
) -> Result<Vec<Album>> {
    let order = match list_type {
        "alphabeticalByArtist" => "ORDER BY artist COLLATE NOCASE, title COLLATE NOCASE",
        "newest" => "ORDER BY year DESC, title COLLATE NOCASE",
        "random" => "ORDER BY RANDOM()",
        _ => "ORDER BY title COLLATE NOCASE",
    };
    let q = format!(
        "SELECT id, title, artist, artist_id, year, cover_art FROM albums {order} LIMIT ?1 OFFSET ?2"
    );
    let rows = sqlx::query_as::<_, Album>(&q)
        .bind(size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}
