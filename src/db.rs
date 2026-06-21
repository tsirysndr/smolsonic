use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::str::FromStr;

pub type Db = Pool<Sqlite>;

pub async fn init(db_path: &Path) -> Result<Db> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating db parent dir {}", parent.display()))?;
        }
    }

    let url = format!("sqlite://{}", db_path.display());
    let opts = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .with_context(|| format!("opening sqlite at {}", db_path.display()))?;

    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &Db) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS artists (
            id           TEXT PRIMARY KEY,
            name         TEXT NOT NULL,
            name_lower   TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS albums (
            id           TEXT PRIMARY KEY,
            title        TEXT NOT NULL,
            artist       TEXT NOT NULL,
            artist_id    TEXT NOT NULL,
            year         INTEGER NOT NULL DEFAULT 0,
            cover_art    TEXT,
            FOREIGN KEY (artist_id) REFERENCES artists(id)
        );

        CREATE INDEX IF NOT EXISTS idx_albums_artist_id ON albums(artist_id);
        CREATE INDEX IF NOT EXISTS idx_albums_title ON albums(title);

        CREATE TABLE IF NOT EXISTS songs (
            id            TEXT PRIMARY KEY,
            path          TEXT NOT NULL UNIQUE,
            title         TEXT NOT NULL,
            artist        TEXT NOT NULL,
            artist_id     TEXT NOT NULL,
            album         TEXT NOT NULL,
            album_id      TEXT NOT NULL,
            genre         TEXT,
            track_number  INTEGER,
            disc_number   INTEGER,
            year          INTEGER,
            duration_ms   INTEGER NOT NULL DEFAULT 0,
            bitrate       INTEGER NOT NULL DEFAULT 0,
            filesize      INTEGER NOT NULL DEFAULT 0,
            suffix        TEXT NOT NULL,
            content_type  TEXT NOT NULL,
            cover_art     TEXT,
            mtime         INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (album_id) REFERENCES albums(id),
            FOREIGN KEY (artist_id) REFERENCES artists(id)
        );

        CREATE INDEX IF NOT EXISTS idx_songs_album_id ON songs(album_id);
        CREATE INDEX IF NOT EXISTS idx_songs_artist_id ON songs(artist_id);
        CREATE INDEX IF NOT EXISTS idx_songs_title ON songs(title);
        CREATE INDEX IF NOT EXISTS idx_songs_genre ON songs(genre);

        CREATE TABLE IF NOT EXISTS starred (
            id          TEXT PRIMARY KEY,
            starred_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS playlists (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            comment     TEXT,
            public      INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS playlist_songs (
            playlist_id TEXT NOT NULL,
            position    INTEGER NOT NULL,
            song_id     TEXT NOT NULL,
            PRIMARY KEY (playlist_id, position),
            FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_playlist_songs_pl ON playlist_songs(playlist_id);
        "#,
    )
    .execute(pool)
    .await
    .context("running migrations")?;
    Ok(())
}
