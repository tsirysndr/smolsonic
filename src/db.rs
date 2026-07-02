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

        -- Backing store for Jellyfin's UserItemDataDto (except `IsFavorite`,
        -- which lives in `starred`). smolsonic is single-user, so we key by
        -- the native item id and skip a user_id column. `likes` is nullable:
        -- NULL = unset, 1 = thumbs-up, 0 = thumbs-down.
        CREATE TABLE IF NOT EXISTS user_item_data (
            id                      TEXT PRIMARY KEY,
            played                  INTEGER NOT NULL DEFAULT 0,
            play_count              INTEGER NOT NULL DEFAULT 0,
            playback_position_ticks INTEGER NOT NULL DEFAULT 0,
            last_played_date        TEXT,
            rating                  REAL,
            likes                   INTEGER
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

        -- ── Videos ──────────────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS videos (
            id            TEXT PRIMARY KEY,
            path          TEXT NOT NULL UNIQUE,
            title         TEXT NOT NULL,
            container     TEXT NOT NULL,
            duration_ms   INTEGER NOT NULL DEFAULT 0,
            filesize      INTEGER NOT NULL DEFAULT 0,
            bitrate       INTEGER NOT NULL DEFAULT 0,
            width         INTEGER NOT NULL DEFAULT 0,
            height        INTEGER NOT NULL DEFAULT 0,
            poster_path   TEXT,
            mtime         INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_videos_title ON videos(title);

        -- ── Jellyfin sidecar ────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS jellyfin_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS jellyfin_tokens (
            token       TEXT PRIMARY KEY,
            user_id     TEXT NOT NULL,
            device_id   TEXT,
            device_name TEXT,
            client      TEXT,
            created_at  TEXT NOT NULL
        );

        -- Reverse lookup from a 32-char hex GUID we emit to Jellyfin clients
        -- back to the native Subsonic-style id (ar-…, al-…, so-…).
        CREATE TABLE IF NOT EXISTS jf_guids (
            guid      TEXT PRIMARY KEY,
            kind      TEXT NOT NULL,
            native_id TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_jf_guids_native ON jf_guids(kind, native_id);

        -- ── FTS5 full-text search ────────────────────────────────────────────
        CREATE VIRTUAL TABLE IF NOT EXISTS songs_fts USING fts5(
            id UNINDEXED, title, artist, album, genre,
            tokenize = 'unicode61 remove_diacritics 2'
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS albums_fts USING fts5(
            id UNINDEXED, title, artist,
            tokenize = 'unicode61 remove_diacritics 2'
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS artists_fts USING fts5(
            id UNINDEXED, name,
            tokenize = 'unicode61 remove_diacritics 2'
        );

        -- Triggers to keep FTS tables in sync with base tables.
        CREATE TRIGGER IF NOT EXISTS songs_ai AFTER INSERT ON songs BEGIN
            INSERT INTO songs_fts (id, title, artist, album, genre)
            VALUES (NEW.id, NEW.title, NEW.artist, NEW.album, COALESCE(NEW.genre, ''));
        END;

        CREATE TRIGGER IF NOT EXISTS songs_ad AFTER DELETE ON songs BEGIN
            DELETE FROM songs_fts WHERE id = OLD.id;
        END;

        CREATE TRIGGER IF NOT EXISTS songs_au AFTER UPDATE ON songs BEGIN
            DELETE FROM songs_fts WHERE id = OLD.id;
            INSERT INTO songs_fts (id, title, artist, album, genre)
            VALUES (NEW.id, NEW.title, NEW.artist, NEW.album, COALESCE(NEW.genre, ''));
        END;

        CREATE TRIGGER IF NOT EXISTS albums_ai AFTER INSERT ON albums BEGIN
            INSERT INTO albums_fts (id, title, artist)
            VALUES (NEW.id, NEW.title, NEW.artist);
        END;

        CREATE TRIGGER IF NOT EXISTS albums_ad AFTER DELETE ON albums BEGIN
            DELETE FROM albums_fts WHERE id = OLD.id;
        END;

        CREATE TRIGGER IF NOT EXISTS albums_au AFTER UPDATE ON albums BEGIN
            DELETE FROM albums_fts WHERE id = OLD.id;
            INSERT INTO albums_fts (id, title, artist)
            VALUES (NEW.id, NEW.title, NEW.artist);
        END;

        CREATE TRIGGER IF NOT EXISTS artists_ai AFTER INSERT ON artists BEGIN
            INSERT INTO artists_fts (id, name) VALUES (NEW.id, NEW.name);
        END;

        CREATE TRIGGER IF NOT EXISTS artists_ad AFTER DELETE ON artists BEGIN
            DELETE FROM artists_fts WHERE id = OLD.id;
        END;

        CREATE TRIGGER IF NOT EXISTS artists_au AFTER UPDATE ON artists BEGIN
            DELETE FROM artists_fts WHERE id = OLD.id;
            INSERT INTO artists_fts (id, name) VALUES (NEW.id, NEW.name);
        END;
        "#,
    )
    .execute(pool)
    .await
    .context("running migrations")?;

    // One-shot backfill for databases that existed before FTS was introduced.
    backfill_fts(pool).await?;
    Ok(())
}

async fn backfill_fts(pool: &Db) -> Result<()> {
    let songs_empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM songs_fts")
        .fetch_one(pool)
        .await?;
    if songs_empty == 0 {
        sqlx::query(
            "INSERT INTO songs_fts (id, title, artist, album, genre)
             SELECT id, title, artist, album, COALESCE(genre, '') FROM songs",
        )
        .execute(pool)
        .await?;
    }
    let albums_empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM albums_fts")
        .fetch_one(pool)
        .await?;
    if albums_empty == 0 {
        sqlx::query(
            "INSERT INTO albums_fts (id, title, artist)
             SELECT id, title, artist FROM albums",
        )
        .execute(pool)
        .await?;
    }
    let artists_empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artists_fts")
        .fetch_one(pool)
        .await?;
    if artists_empty == 0 {
        sqlx::query("INSERT INTO artists_fts (id, name) SELECT id, name FROM artists")
            .execute(pool)
            .await?;
    }
    Ok(())
}
