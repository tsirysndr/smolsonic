use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: String,
    pub year: i64,
    pub cover_art: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub public: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct InternetRadioStation {
    pub id: String,
    pub name: String,
    pub stream_url: String,
    pub homepage_url: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Video {
    pub id: String,
    pub path: String,
    pub title: String,
    pub container: String,
    pub duration_ms: i64,
    pub filesize: i64,
    pub bitrate: i64,
    pub width: i64,
    pub height: i64,
    pub poster_path: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Song {
    pub id: String,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub artist_id: String,
    pub album: String,
    pub album_id: String,
    pub genre: Option<String>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub year: Option<i64>,
    pub duration_ms: i64,
    pub bitrate: i64,
    pub filesize: i64,
    pub suffix: String,
    pub content_type: String,
    pub cover_art: Option<String>,
}
