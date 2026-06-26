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
    pub mdns: MdnsConfig,
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
        Ok(cfg)
    }
}
