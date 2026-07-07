use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub music_dir: PathBuf,
    pub username: String,
    pub password: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_db_path")]
    pub database_path: PathBuf,
    #[serde(default = "default_covers_dir")]
    pub covers_dir: PathBuf,
    #[serde(default = "default_scan_interval_secs")]
    pub scan_interval_secs: u64,
    #[serde(default)]
    pub s3: Option<S3Config>,
    #[serde(default)]
    pub jellyfin: Option<JellyfinConfig>,
    #[serde(default)]
    pub video: Option<VideoConfig>,
    #[serde(default)]
    pub mdns: MdnsConfig,
    /// Optional Last.fm plugin. Enabled only when the block exists and
    /// `api_key` is set — powers `/…/Similar` via `artist.getSimilar`.
    #[serde(default)]
    pub lastfm: Option<LastfmConfig>,
    /// Optional MusicBrainz plugin. Enabled only when the block exists and
    /// `user_agent` is set (MB's TOS requires a descriptive UA per app).
    /// Supplements Last.fm with band-member / collaborator links.
    #[serde(default)]
    pub musicbrainz: Option<MusicbrainzConfig>,
    /// Optional Typesense search backend. When present, the free-text
    /// `search3` / `search2` / Jellyfin `?searchTerm=` endpoints use Typesense
    /// instead of the built-in SQLite FTS5 index. Omit the block to keep
    /// using FTS5.
    #[serde(default)]
    pub typesense: Option<TypesenseConfig>,
    /// Optional ListenBrainz scrobble target. When present, playback events
    /// (Jellyfin `/Sessions/Playing[/Stopped]` and Subsonic `/rest/scrobble`)
    /// submit `playing_now` + `single` listens to ListenBrainz. Follows the
    /// same rules Last.fm's scrobble spec uses: track > 30s AND
    /// (position > half OR position > 4 min) qualifies as a scrobble.
    #[serde(default)]
    pub listenbrainz: Option<ListenBrainzConfig>,
    /// Optional UPnP/DLNA media server. When present (and `enabled`), the
    /// library is announced over SSDP and browsable from DLNA renderers and
    /// control points (VLC, BubbleUPnP, smart TVs, Sonos, …). Streams are
    /// served unauthenticated on the UPnP port — DLNA has no auth concept —
    /// so only enable this on trusted networks.
    #[serde(default)]
    pub upnp: Option<UpnpConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpnpConfig {
    #[serde(default = "default_upnp_enabled")]
    pub enabled: bool,
    #[serde(default = "default_upnp_host")]
    pub host: String,
    #[serde(default = "default_upnp_port")]
    pub port: u16,
    #[serde(default = "default_upnp_friendly_name")]
    pub friendly_name: String,
}

fn default_upnp_enabled() -> bool {
    true
}

fn default_upnp_host() -> String {
    "0.0.0.0".to_string()
}

fn default_upnp_port() -> u16 {
    8200
}

fn default_upnp_friendly_name() -> String {
    "smolsonic".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenBrainzConfig {
    /// User token from https://listenbrainz.org/profile.
    pub token: String,
    /// API base URL. Defaults to the official ListenBrainz instance; set to
    /// your own for a self-hosted deployment.
    #[serde(default = "default_listenbrainz_api_url")]
    pub api_url: String,
}

fn default_listenbrainz_api_url() -> String {
    "https://api.listenbrainz.org".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct LastfmConfig {
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MusicbrainzConfig {
    pub user_agent: String,
}

/// Optional Typesense search backend. `url` points at the Typesense HTTP
/// endpoint (no trailing slash); `api_key` is the admin key so smolsonic can
/// create collections and import documents. `collection_prefix` lets multiple
/// smolsonic instances share a single Typesense node without colliding.
#[derive(Debug, Clone, Deserialize)]
pub struct TypesenseConfig {
    pub url: String,
    pub api_key: String,
    #[serde(default = "default_typesense_prefix")]
    pub collection_prefix: String,
}

fn default_typesense_prefix() -> String {
    "smolsonic".to_string()
}

/// Optional video library. Enabled only when this block is present in the
/// TOML and `video_dir` is set. Surfaces through the Jellyfin sidecar as a
/// second collection (`CollectionType="movies"`).
#[derive(Debug, Clone, Deserialize)]
pub struct VideoConfig {
    pub video_dir: PathBuf,
    #[serde(default = "default_video_scan_interval_secs")]
    pub scan_interval_secs: u64,
    #[serde(default = "default_video_library_name")]
    pub library_name: String,
}

fn default_video_scan_interval_secs() -> u64 {
    300
}

fn default_video_library_name() -> String {
    "Movies".to_string()
}

/// Jellyfin sidecar API. Enabled only when this block exists and `port` is
/// set in the TOML. Omit the block entirely to disable.
#[derive(Debug, Clone, Deserialize)]
pub struct JellyfinConfig {
    pub port: u16,
    #[serde(default = "default_jellyfin_host")]
    pub host: String,
    #[serde(default = "default_jellyfin_server_name")]
    pub server_name: String,
}

fn default_jellyfin_host() -> String {
    "0.0.0.0".to_string()
}

fn default_jellyfin_server_name() -> String {
    "smolsonic".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct MdnsConfig {
    #[serde(default = "default_mdns_enabled")]
    pub enabled: bool,
    #[serde(default = "default_mdns_instance")]
    pub instance_name: String,
}

impl Default for MdnsConfig {
    fn default() -> Self {
        Self {
            enabled: default_mdns_enabled(),
            instance_name: default_mdns_instance(),
        }
    }
}

fn default_mdns_enabled() -> bool {
    true
}

fn default_mdns_instance() -> String {
    "smolsonic".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct S3Config {
    #[serde(default = "default_s3_enabled")]
    pub enabled: bool,
    #[serde(default = "default_s3_host")]
    pub host: String,
    #[serde(default = "default_s3_port")]
    pub port: u16,
    pub access_key: String,
    pub secret_key: String,
}

fn default_s3_enabled() -> bool {
    true
}

fn default_s3_host() -> String {
    "0.0.0.0".to_string()
}

fn default_s3_port() -> u16 {
    9000
}

fn default_port() -> u16 {
    4533
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_db_path() -> PathBuf {
    PathBuf::from("smolsonic.db")
}

fn default_covers_dir() -> PathBuf {
    PathBuf::from("covers")
}

fn default_scan_interval_secs() -> u64 {
    300
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        if cfg.password.is_empty() {
            anyhow::bail!("config: password must not be empty");
        }
        if cfg.username.is_empty() {
            anyhow::bail!("config: username must not be empty");
        }
        if let Some(s3) = &cfg.s3 {
            if s3.enabled {
                if s3.access_key.is_empty() {
                    anyhow::bail!("config: s3.access_key must not be empty");
                }
                if s3.secret_key.is_empty() {
                    anyhow::bail!("config: s3.secret_key must not be empty");
                }
            }
        }
        if let Some(ts) = &cfg.typesense {
            if ts.url.is_empty() {
                anyhow::bail!("config: typesense.url must not be empty");
            }
            if ts.api_key.is_empty() {
                anyhow::bail!("config: typesense.api_key must not be empty");
            }
        }
        if let Some(lb) = &cfg.listenbrainz {
            if lb.token.is_empty() {
                anyhow::bail!("config: listenbrainz.token must not be empty");
            }
            if lb.api_url.is_empty() {
                anyhow::bail!("config: listenbrainz.api_url must not be empty");
            }
        }
        Ok(cfg)
    }
}
