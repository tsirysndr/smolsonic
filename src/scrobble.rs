//! ListenBrainz scrobble client.
//!
//! Opt-in: constructed from `[listenbrainz]` in `smolsonic.toml`. When the
//! block is absent, callers hold `None` and every entry point becomes a
//! no-op.
//!
//! Scrobble semantics match the Last.fm submission rules (which ListenBrainz
//! also honours):
//!   1. Track must be longer than 30 seconds.
//!   2. Track has been played past the halfway point OR for at least 4
//!      minutes, whichever comes first.
//!
//! Both entry points return `()` — network / auth errors are logged at
//! `tracing::warn!` and swallowed. A ListenBrainz outage must never stop a
//! song from playing.

use anyhow::Result;
use serde::Serialize;

/// Minimum track length (seconds) for a listen to be scrobbleable. Below
/// this threshold Last.fm never counts a play — we follow the same rule so
/// audiobook chapters and podcast intros don't inflate stats.
pub const MIN_SCROBBLE_DURATION_SECS: i64 = 30;

/// Cap on the "played long enough" check: once a listener has played four
/// minutes of a track, the scrobble qualifies even if the halfway point
/// hasn't been reached (Last.fm rule, adopted by ListenBrainz clients).
pub const SCROBBLE_MIN_PLAYED_SECS: i64 = 240;

/// True when the (position, duration) pair satisfies the Last.fm scrobble
/// rules — extracted so tests can pin the exact behaviour without a live
/// HTTP round trip.
pub fn qualifies_for_scrobble(position_secs: i64, duration_secs: i64) -> bool {
    if duration_secs <= MIN_SCROBBLE_DURATION_SECS {
        return false;
    }
    let half = duration_secs / 2;
    position_secs >= SCROBBLE_MIN_PLAYED_SECS || position_secs >= half
}

pub struct ListenBrainzClient {
    token: String,
    api_url: String,
    http: reqwest::Client,
}

impl ListenBrainzClient {
    pub fn new(cfg: &crate::config::ListenBrainzConfig) -> Self {
        Self {
            token: cfg.token.clone(),
            api_url: cfg.api_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .expect("build reqwest client"),
        }
    }

    /// `POST /1/submit-listens` with `listen_type=playing_now`. Called at
    /// playback start; ListenBrainz treats it as a live-status update and
    /// doesn't persist it to the user's history.
    pub async fn submit_playing_now(&self, meta: TrackMeta<'_>) {
        let body = SubmitListensBody {
            listen_type: "playing_now",
            payload: vec![ListenPayload {
                listened_at: None,
                track_metadata: (&meta).into(),
            }],
        };
        if let Err(e) = self.post_listen(&body).await {
            tracing::warn!("listenbrainz playing_now({}): {e}", meta.track);
        }
    }

    /// `POST /1/submit-listens` with `listen_type=single`. Called at
    /// playback stop when the scrobble rules are met. `listened_at` is a
    /// unix timestamp representing playback START, per ListenBrainz's own
    /// documentation.
    pub async fn submit_listen(&self, meta: TrackMeta<'_>, listened_at: i64) {
        let body = SubmitListensBody {
            listen_type: "single",
            payload: vec![ListenPayload {
                listened_at: Some(listened_at),
                track_metadata: (&meta).into(),
            }],
        };
        if let Err(e) = self.post_listen(&body).await {
            tracing::warn!("listenbrainz submit_listen({}): {e}", meta.track);
        }
    }

    async fn post_listen(&self, body: &SubmitListensBody<'_>) -> Result<()> {
        let url = format!("{}/1/submit-listens", self.api_url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Token {}", self.token))
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("HTTP {status}: {text}");
        }
        Ok(())
    }
}

/// Metadata for a single track. Owned by the caller; we only borrow.
#[derive(Debug, Clone, Copy)]
pub struct TrackMeta<'a> {
    pub artist: &'a str,
    pub track: &'a str,
    pub album: Option<&'a str>,
}

// ── Wire format ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SubmitListensBody<'a> {
    listen_type: &'static str,
    payload: Vec<ListenPayload<'a>>,
}

#[derive(Debug, Serialize)]
struct ListenPayload<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    listened_at: Option<i64>,
    track_metadata: TrackMetaBody<'a>,
}

#[derive(Debug, Serialize)]
struct TrackMetaBody<'a> {
    artist_name: &'a str,
    track_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_name: Option<&'a str>,
}

impl<'a> From<&TrackMeta<'a>> for TrackMetaBody<'a> {
    fn from(m: &TrackMeta<'a>) -> Self {
        Self {
            artist_name: m.artist,
            track_name: m.track,
            release_name: m.album,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_tracks_never_qualify() {
        // 30-second boundary: not qualifying even if fully played.
        assert!(!qualifies_for_scrobble(30, 30));
        assert!(!qualifies_for_scrobble(29, 29));
    }

    #[test]
    fn halfway_point_qualifies_for_medium_track() {
        // 3-minute track: halfway = 90s.
        assert!(!qualifies_for_scrobble(89, 180));
        assert!(qualifies_for_scrobble(90, 180));
    }

    #[test]
    fn four_minute_cap_qualifies_for_long_track() {
        // 20-minute track: halfway is 10m, but 4m cap wins.
        assert!(!qualifies_for_scrobble(239, 20 * 60));
        assert!(qualifies_for_scrobble(240, 20 * 60));
    }

    #[test]
    fn serialises_playing_now_without_listened_at() {
        let body = SubmitListensBody {
            listen_type: "playing_now",
            payload: vec![ListenPayload {
                listened_at: None,
                track_metadata: TrackMetaBody {
                    artist_name: "Artist",
                    track_name: "Track",
                    release_name: Some("Album"),
                },
            }],
        };
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains(r#""listen_type":"playing_now""#));
        assert!(!s.contains("listened_at"));
        assert!(s.contains(r#""release_name":"Album""#));
    }

    #[test]
    fn serialises_single_with_listened_at_and_no_album() {
        let body = SubmitListensBody {
            listen_type: "single",
            payload: vec![ListenPayload {
                listened_at: Some(1_720_000_000),
                track_metadata: TrackMetaBody {
                    artist_name: "Artist",
                    track_name: "Track",
                    release_name: None,
                },
            }],
        };
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains(r#""listened_at":1720000000"#));
        assert!(!s.contains("release_name"));
    }
}
