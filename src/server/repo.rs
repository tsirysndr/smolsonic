use crate::db::Db;
use crate::models::{Album, Artist, Playlist, Song};
use anyhow::Result;

pub async fn all_artists(pool: &Db) -> Result<Vec<Artist>> {
    let rows = sqlx::query_as::<_, Artist>(
        "SELECT id, name FROM artists ORDER BY name COLLATE NOCASE",
    )
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
    let total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(duration_ms), 0) FROM songs WHERE album_id = ?1",
    )
    .bind(album_id)
    .fetch_one(pool)
    .await?;
    Ok(total / 1000)
}

pub async fn song_count_for_album(pool: &Db, album_id: &str) -> Result<i64> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM songs WHERE album_id = ?1")
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

pub async fn songs_by_genre(
    pool: &Db,
    genre: &str,
    count: i64,
    offset: i64,
) -> Result<Vec<Song>> {
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

pub async fn set_playlist_comment(
    pool: &Db,
    id: &str,
    comment: &str,
    now: &str,
) -> Result<()> {
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
