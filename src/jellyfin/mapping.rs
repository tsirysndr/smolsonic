use crate::db::Db;
use crate::models::{Album, Artist, Song, Video};
use anyhow::Result;
use sha2::{Digest, Sha256};

pub const KIND_ARTIST: &str = "artist";
pub const KIND_ALBUM: &str = "album";
pub const KIND_SONG: &str = "song";
pub const KIND_VIDEO: &str = "video";
pub const KIND_LIBRARY: &str = "library";
pub const KIND_USER: &str = "user";

/// Stable per (kind, native_id) GUID, formatted as a dashed UUID
/// (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`). Jellyfin clients built on the
/// official Kotlin/Java SDKs parse this via `UUID.fromString()`, which
/// rejects the un-dashed 32-char hex form — emitting plain hex caused
/// Findroid to drop our user with "no users found".
pub fn guid(kind: &str, native_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b":");
    h.update(native_id.as_bytes());
    let digest = h.finalize();
    format_as_uuid(&hex::encode(&digest[..16]))
}

/// Format a 32-char hex string as a dashed UUID. Pass-through for any other
/// length so callers can blindly pipe random strings through.
pub fn guid_dashed(hex32: &str) -> String {
    format_as_uuid(hex32)
}

fn format_as_uuid(hex32: &str) -> String {
    if hex32.len() != 32 {
        return hex32.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex32[0..8],
        &hex32[8..12],
        &hex32[12..16],
        &hex32[16..20],
        &hex32[20..32],
    )
}

/// Accept either dashed or un-dashed GUID forms from clients and return the
/// canonical dashed form we store in `jf_guids`. Lower-cased so lookups are
/// case-insensitive.
pub fn normalize_guid(input: &str) -> String {
    let stripped: String = input
        .chars()
        .filter(|c| *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();
    if stripped.len() == 32 && stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        format_as_uuid(&stripped)
    } else {
        input.to_ascii_lowercase()
    }
}

/// Record a (guid → native_id) pair so we can reverse the lookup later when a
/// client requests `/Items/{guid}`. Cheap upsert; safe to call on every emit.
pub async fn remember(pool: &Db, kind: &str, native_id: &str) -> Result<String> {
    let g = guid(kind, native_id);
    sqlx::query(
        "INSERT INTO jf_guids (guid, kind, native_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(guid) DO NOTHING",
    )
    .bind(&g)
    .bind(kind)
    .bind(native_id)
    .execute(pool)
    .await?;
    Ok(g)
}

pub async fn lookup(pool: &Db, guid: &str) -> Result<Option<(String, String)>> {
    let g = normalize_guid(guid);
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT kind, native_id FROM jf_guids WHERE guid = ?1")
            .bind(&g)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

pub async fn remember_artist(pool: &Db, a: &Artist) -> Result<String> {
    remember(pool, KIND_ARTIST, &a.id).await
}

pub async fn remember_album(pool: &Db, a: &Album) -> Result<String> {
    remember(pool, KIND_ALBUM, &a.id).await
}

pub async fn remember_song(pool: &Db, s: &Song) -> Result<String> {
    remember(pool, KIND_SONG, &s.id).await
}

pub async fn remember_video(pool: &Db, v: &Video) -> Result<String> {
    remember(pool, KIND_VIDEO, &v.id).await
}

/// Deterministic GUID for the music + movies virtual libraries.
pub fn library_guid() -> String {
    guid(KIND_LIBRARY, "music")
}

pub fn movies_library_guid() -> String {
    guid(KIND_LIBRARY, "movies")
}

pub fn user_guid(username: &str) -> String {
    guid(KIND_USER, username)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[test]
    fn guid_is_uuid_formatted_and_stable() {
        let a = guid(KIND_ARTIST, "ar-foo");
        let b = guid(KIND_ARTIST, "ar-foo");
        assert_eq!(a, b);
        assert_eq!(a.len(), 36, "expected dashed UUID, got {a:?}");
        // 4 dashes at positions 8, 13, 18, 23.
        assert_eq!(a.as_bytes()[8], b'-');
        assert_eq!(a.as_bytes()[13], b'-');
        assert_eq!(a.as_bytes()[18], b'-');
        assert_eq!(a.as_bytes()[23], b'-');
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
    }

    #[test]
    fn guid_differs_by_kind() {
        assert_ne!(guid(KIND_ARTIST, "x"), guid(KIND_ALBUM, "x"));
    }

    #[test]
    fn guid_dashed_formats_correctly() {
        let g = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            guid_dashed(g),
            "01234567-89ab-cdef-0123-456789abcdef"
        );
    }

    #[test]
    fn guid_dashed_passthrough_for_wrong_length() {
        assert_eq!(guid_dashed("short"), "short");
    }

    #[test]
    fn normalize_guid_returns_dashed_form_for_both_inputs() {
        let dashed = "01234567-89ab-cdef-0123-456789abcdef";
        assert_eq!(
            normalize_guid("01234567-89AB-CDEF-0123-456789ABCDEF"),
            dashed
        );
        assert_eq!(
            normalize_guid("0123456789abcdef0123456789abcdef"),
            dashed
        );
    }

    #[tokio::test]
    async fn remember_then_lookup_roundtrip() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE jf_guids (
                guid TEXT PRIMARY KEY, kind TEXT NOT NULL, native_id TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let g = remember(&pool, KIND_SONG, "so-123").await.unwrap();
        let found = lookup(&pool, &g).await.unwrap();
        assert_eq!(found, Some((KIND_SONG.to_string(), "so-123".to_string())));
    }

    #[tokio::test]
    async fn lookup_accepts_dashed_guid() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE jf_guids (
                guid TEXT PRIMARY KEY, kind TEXT NOT NULL, native_id TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let g = remember(&pool, KIND_ALBUM, "al-9").await.unwrap();
        let dashed = guid_dashed(&g);
        let found = lookup(&pool, &dashed).await.unwrap();
        assert_eq!(found, Some((KIND_ALBUM.to_string(), "al-9".to_string())));
    }
}
