use crate::db::Db;
use crate::models::{Album, Artist, Song};
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

pub async fn search_artists(pool: &Db, term: &str, limit: i64, offset: i64) -> Result<Vec<Artist>> {
    let pat = format!("%{}%", term);
    let rows = sqlx::query_as::<_, Artist>(
        "SELECT id, name FROM artists WHERE name LIKE ?1 COLLATE NOCASE
         ORDER BY name COLLATE NOCASE LIMIT ?2 OFFSET ?3",
    )
    .bind(pat)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn search_albums(pool: &Db, term: &str, limit: i64, offset: i64) -> Result<Vec<Album>> {
    let pat = format!("%{}%", term);
    let rows = sqlx::query_as::<_, Album>(
        "SELECT id, title, artist, artist_id, year, cover_art FROM albums
         WHERE title LIKE ?1 COLLATE NOCASE OR artist LIKE ?1 COLLATE NOCASE
         ORDER BY title COLLATE NOCASE LIMIT ?2 OFFSET ?3",
    )
    .bind(pat)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn search_songs(pool: &Db, term: &str, limit: i64, offset: i64) -> Result<Vec<Song>> {
    let pat = format!("%{}%", term);
    let rows = sqlx::query_as::<_, Song>(
        "SELECT id, path, title, artist, artist_id, album, album_id, genre, track_number,
                disc_number, year, duration_ms, bitrate, filesize, suffix, content_type, cover_art
         FROM songs
         WHERE title LIKE ?1 COLLATE NOCASE OR artist LIKE ?1 COLLATE NOCASE OR album LIKE ?1 COLLATE NOCASE
         ORDER BY title COLLATE NOCASE LIMIT ?2 OFFSET ?3",
    )
    .bind(pat)
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
