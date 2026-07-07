//! ContentDirectory:1 control endpoint — the SOAP Browse tree that DLNA
//! control points walk:
//!
//! ```text
//! 0 (root)
//! ├── artists   → artist:{ar-…} → album:{al-…} → track:{so-…}
//! ├── albums    → album:{al-…}  → track:{so-…}
//! └── playlists → playlist:{pl-…} → track:{so-…}
//! ```
//!
//! SOAP bodies here are tiny and rigidly shaped, so arguments are pulled out
//! with a tag scanner instead of a full XML parser dependency.

use super::{xml_escape, UpnpState, CD_SERVICE_TYPE, CM_SERVICE_TYPE, SERVER_HEADER};
use crate::models::Song;
use crate::server::repo;
use actix_web::{web, HttpRequest, HttpResponse};
use std::sync::Arc;

const ROOT_ID: &str = "0";
const ARTISTS_ID: &str = "artists";
const ALBUMS_ID: &str = "albums";
const PLAYLISTS_ID: &str = "playlists";

pub async fn control(
    state: web::Data<Arc<UpnpState>>,
    req: HttpRequest,
    body: web::Bytes,
) -> HttpResponse {
    let action = soap_action(&req);
    let body = String::from_utf8_lossy(&body).into_owned();
    match action.as_deref() {
        Some("Browse") => browse(&state, &req, &body).await,
        Some("GetSystemUpdateID") => soap_ok(
            CD_SERVICE_TYPE,
            "GetSystemUpdateID",
            "<Id>1</Id>".to_string(),
        ),
        Some("GetSortCapabilities") => soap_ok(
            CD_SERVICE_TYPE,
            "GetSortCapabilities",
            "<SortCaps></SortCaps>".to_string(),
        ),
        Some("GetSearchCapabilities") => soap_ok(
            CD_SERVICE_TYPE,
            "GetSearchCapabilities",
            "<SearchCaps></SearchCaps>".to_string(),
        ),
        // We advertise no search capabilities, but answer a stray Search
        // with an empty result rather than a fault.
        Some("Search") => soap_ok(
            CD_SERVICE_TYPE,
            "Search",
            format!(
                "<Result>{}</Result><NumberReturned>0</NumberReturned><TotalMatches>0</TotalMatches><UpdateID>1</UpdateID>",
                xml_escape(&didl(String::new()))
            ),
        ),
        _ => soap_fault(401, "Invalid Action"),
    }
}

pub async fn connection_manager_control(req: HttpRequest, _body: web::Bytes) -> HttpResponse {
    match soap_action(&req).as_deref() {
        Some("GetProtocolInfo") => {
            let source = [
                "audio/mpeg",
                "audio/flac",
                "audio/x-flac",
                "audio/ogg",
                "audio/mp4",
                "audio/aac",
                "audio/x-m4a",
                "audio/wav",
                "audio/x-wav",
                "audio/x-aiff",
                "audio/x-ms-wma",
                "audio/x-musepack",
                "audio/x-ape",
                "audio/x-wavpack",
            ]
            .iter()
            .map(|m| format!("http-get:*:{m}:*"))
            .collect::<Vec<_>>()
            .join(",");
            soap_ok(
                CM_SERVICE_TYPE,
                "GetProtocolInfo",
                format!("<Source>{}</Source><Sink></Sink>", xml_escape(&source)),
            )
        }
        Some("GetCurrentConnectionIDs") => soap_ok(
            CM_SERVICE_TYPE,
            "GetCurrentConnectionIDs",
            "<ConnectionIDs>0</ConnectionIDs>".to_string(),
        ),
        Some("GetCurrentConnectionInfo") => soap_ok(
            CM_SERVICE_TYPE,
            "GetCurrentConnectionInfo",
            "<RcsID>-1</RcsID><AVTransportID>-1</AVTransportID><ProtocolInfo></ProtocolInfo>\
             <PeerConnectionManager></PeerConnectionManager><PeerConnectionID>-1</PeerConnectionID>\
             <Direction>Output</Direction><Status>OK</Status>"
                .to_string(),
        ),
        _ => soap_fault(401, "Invalid Action"),
    }
}

async fn browse(state: &UpnpState, req: &HttpRequest, body: &str) -> HttpResponse {
    let object_id = soap_arg(body, "ObjectID").unwrap_or_else(|| ROOT_ID.to_string());
    let browse_flag =
        soap_arg(body, "BrowseFlag").unwrap_or_else(|| "BrowseDirectChildren".to_string());
    let start: usize = soap_arg(body, "StartingIndex")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let count: usize = soap_arg(body, "RequestedCount")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let base = format!("http://{}", req.connection_info().host());
    let result = if browse_flag == "BrowseMetadata" {
        browse_metadata(state, &object_id, &base).await
    } else {
        browse_children(state, &object_id, &base, start, count).await
    };

    match result {
        Ok((entries, total)) => {
            let returned = entries.matches("<item").count() + entries.matches("<container").count();
            soap_ok(
                CD_SERVICE_TYPE,
                "Browse",
                format!(
                    "<Result>{}</Result><NumberReturned>{}</NumberReturned><TotalMatches>{}</TotalMatches><UpdateID>1</UpdateID>",
                    xml_escape(&didl(entries)),
                    returned,
                    total
                ),
            )
        }
        Err(BrowseError::NoSuchObject) => soap_fault(701, "No such object"),
        Err(BrowseError::Db(e)) => {
            tracing::error!("upnp browse {object_id}: {e}");
            soap_fault(501, "Action Failed")
        }
    }
}

enum BrowseError {
    NoSuchObject,
    Db(anyhow::Error),
}

impl From<anyhow::Error> for BrowseError {
    fn from(e: anyhow::Error) -> Self {
        BrowseError::Db(e)
    }
}

/// Returns (DIDL fragments, TotalMatches).
async fn browse_children(
    state: &UpnpState,
    object_id: &str,
    base: &str,
    start: usize,
    count: usize,
) -> Result<(String, usize), BrowseError> {
    let mut out = String::new();
    match object_id {
        ROOT_ID => {
            let artists = repo::count_artists(&state.pool).await?;
            let albums = repo::count_albums(&state.pool).await?;
            let playlists = repo::all_playlists(&state.pool).await?.len() as i64;
            let tops = [
                (ARTISTS_ID, "Artists", artists),
                (ALBUMS_ID, "Albums", albums),
                (PLAYLISTS_ID, "Playlists", playlists),
            ];
            let total = tops.len();
            for (id, title, child_count) in page(&tops, start, count) {
                container(
                    &mut out,
                    id,
                    ROOT_ID,
                    title,
                    "object.container.storageFolder",
                    Some(*child_count),
                    None,
                );
            }
            Ok((out, total))
        }
        ARTISTS_ID => {
            let artists = repo::all_artists(&state.pool).await?;
            let counts = repo::album_counts_by_artist(&state.pool).await?;
            let total = artists.len();
            for a in page(&artists, start, count) {
                container(
                    &mut out,
                    &format!("artist:{}", a.id),
                    ARTISTS_ID,
                    &a.name,
                    "object.container.person.musicArtist",
                    counts.get(&a.id).copied(),
                    None,
                );
            }
            Ok((out, total))
        }
        ALBUMS_ID => {
            let albums = repo::all_albums(&state.pool).await?;
            let counts = repo::song_counts_by_album(&state.pool).await?;
            let total = albums.len();
            for al in page(&albums, start, count) {
                album_container(&mut out, al, ALBUMS_ID, counts.get(&al.id).copied(), base);
            }
            Ok((out, total))
        }
        PLAYLISTS_ID => {
            let playlists = repo::all_playlists(&state.pool).await?;
            let total = playlists.len();
            for p in page(&playlists, start, count) {
                container(
                    &mut out,
                    &format!("playlist:{}", p.id),
                    PLAYLISTS_ID,
                    &p.name,
                    "object.container.playlistContainer",
                    None,
                    None,
                );
            }
            Ok((out, total))
        }
        _ => {
            if let Some(artist_id) = object_id.strip_prefix("artist:") {
                let albums = repo::albums_by_artist(&state.pool, artist_id).await?;
                let counts = repo::song_counts_by_album(&state.pool).await?;
                let total = albums.len();
                for al in page(&albums, start, count) {
                    album_container(&mut out, al, object_id, counts.get(&al.id).copied(), base);
                }
                Ok((out, total))
            } else if let Some(album_id) = object_id.strip_prefix("album:") {
                let songs = repo::songs_by_album(&state.pool, album_id).await?;
                let total = songs.len();
                for s in page(&songs, start, count) {
                    track_item(&mut out, s, object_id, base);
                }
                Ok((out, total))
            } else if let Some(playlist_id) = object_id.strip_prefix("playlist:") {
                let songs = repo::playlist_songs(&state.pool, playlist_id).await?;
                let total = songs.len();
                for s in page(&songs, start, count) {
                    track_item(&mut out, s, object_id, base);
                }
                Ok((out, total))
            } else if object_id.starts_with("track:") {
                Ok((out, 0))
            } else {
                Err(BrowseError::NoSuchObject)
            }
        }
    }
}

async fn browse_metadata(
    state: &UpnpState,
    object_id: &str,
    base: &str,
) -> Result<(String, usize), BrowseError> {
    let mut out = String::new();
    match object_id {
        ROOT_ID => {
            container(
                &mut out,
                ROOT_ID,
                "-1",
                &state.friendly_name,
                "object.container.storageFolder",
                Some(3),
                None,
            );
        }
        ARTISTS_ID | ALBUMS_ID | PLAYLISTS_ID => {
            let title = match object_id {
                ARTISTS_ID => "Artists",
                ALBUMS_ID => "Albums",
                _ => "Playlists",
            };
            container(
                &mut out,
                object_id,
                ROOT_ID,
                title,
                "object.container.storageFolder",
                None,
                None,
            );
        }
        _ => {
            if let Some(artist_id) = object_id.strip_prefix("artist:") {
                let artist = repo::find_artist(&state.pool, artist_id)
                    .await?
                    .ok_or(BrowseError::NoSuchObject)?;
                container(
                    &mut out,
                    object_id,
                    ARTISTS_ID,
                    &artist.name,
                    "object.container.person.musicArtist",
                    None,
                    None,
                );
            } else if let Some(album_id) = object_id.strip_prefix("album:") {
                let album = repo::find_album(&state.pool, album_id)
                    .await?
                    .ok_or(BrowseError::NoSuchObject)?;
                album_container(&mut out, &album, ALBUMS_ID, None, base);
            } else if let Some(playlist_id) = object_id.strip_prefix("playlist:") {
                let playlist = repo::find_playlist(&state.pool, playlist_id)
                    .await?
                    .ok_or(BrowseError::NoSuchObject)?;
                container(
                    &mut out,
                    object_id,
                    PLAYLISTS_ID,
                    &playlist.name,
                    "object.container.playlistContainer",
                    None,
                    None,
                );
            } else if let Some(song_id) = object_id.strip_prefix("track:") {
                let song = repo::find_song(&state.pool, song_id)
                    .await?
                    .ok_or(BrowseError::NoSuchObject)?;
                let parent = format!("album:{}", song.album_id);
                track_item(&mut out, &song, &parent, base);
            } else {
                return Err(BrowseError::NoSuchObject);
            }
        }
    }
    Ok((out, 1))
}

fn page<T>(items: &[T], start: usize, count: usize) -> &[T] {
    let start = start.min(items.len());
    let end = if count == 0 {
        items.len()
    } else {
        (start + count).min(items.len())
    };
    &items[start..end]
}

fn container(
    out: &mut String,
    id: &str,
    parent: &str,
    title: &str,
    class: &str,
    child_count: Option<i64>,
    extra: Option<&str>,
) {
    let child_attr = child_count
        .map(|n| format!(" childCount=\"{n}\""))
        .unwrap_or_default();
    out.push_str(&format!(
        "<container id=\"{}\" parentID=\"{}\" restricted=\"1\" searchable=\"0\"{}>\
         <dc:title>{}</dc:title><upnp:class>{}</upnp:class>{}</container>",
        xml_escape(id),
        xml_escape(parent),
        child_attr,
        xml_escape(title),
        class,
        extra.unwrap_or_default(),
    ));
}

fn album_container(
    out: &mut String,
    album: &crate::models::Album,
    parent: &str,
    child_count: Option<i64>,
    base: &str,
) {
    let mut extra = format!("<upnp:artist>{}</upnp:artist>", xml_escape(&album.artist));
    extra.push_str(&format!(
        "<dc:creator>{}</dc:creator>",
        xml_escape(&album.artist)
    ));
    if album.year > 0 {
        extra.push_str(&format!("<dc:date>{}-01-01</dc:date>", album.year));
    }
    if album.cover_art.is_some() {
        extra.push_str(&format!(
            "<upnp:albumArtURI>{base}/art/{}</upnp:albumArtURI>",
            xml_escape(&album.id)
        ));
    }
    container(
        out,
        &format!("album:{}", album.id),
        parent,
        &album.title,
        "object.container.album.musicAlbum",
        child_count,
        Some(&extra),
    );
}

fn track_item(out: &mut String, song: &Song, parent: &str, base: &str) {
    let url = format!(
        "{base}/stream/{}.{}",
        xml_escape(&song.id),
        xml_escape(&song.suffix)
    );
    let duration = hms(song.duration_ms);
    // res@bitrate is bytes per second; the DB stores kbit/s.
    let byte_rate = song.bitrate * 1000 / 8;
    let mut meta = format!(
        "<dc:title>{}</dc:title><upnp:class>object.item.audioItem.musicTrack</upnp:class>\
         <dc:creator>{artist}</dc:creator><upnp:artist>{artist}</upnp:artist>\
         <upnp:album>{album}</upnp:album>",
        xml_escape(&song.title),
        artist = xml_escape(&song.artist),
        album = xml_escape(&song.album),
    );
    if let Some(g) = &song.genre {
        meta.push_str(&format!("<upnp:genre>{}</upnp:genre>", xml_escape(g)));
    }
    if let Some(n) = song.track_number {
        meta.push_str(&format!(
            "<upnp:originalTrackNumber>{n}</upnp:originalTrackNumber>"
        ));
    }
    if let Some(y) = song.year {
        meta.push_str(&format!("<dc:date>{y}-01-01</dc:date>"));
    }
    if song.cover_art.is_some() || !song.album_id.is_empty() {
        meta.push_str(&format!(
            "<upnp:albumArtURI>{base}/art/{}</upnp:albumArtURI>",
            xml_escape(&song.album_id)
        ));
    }
    out.push_str(&format!(
        "<item id=\"track:{id}\" parentID=\"{parent}\" restricted=\"1\">{meta}\
         <res protocolInfo=\"http-get:*:{ct}:*\" size=\"{size}\" duration=\"{duration}\" bitrate=\"{byte_rate}\">{url}</res>\
         </item>",
        id = xml_escape(&song.id),
        parent = xml_escape(parent),
        ct = xml_escape(&song.content_type),
        size = song.filesize,
    ));
}

fn hms(ms: i64) -> String {
    let total_secs = ms / 1000;
    format!(
        "{}:{:02}:{:02}",
        total_secs / 3600,
        (total_secs / 60) % 60,
        total_secs % 60
    )
}

fn didl(entries: String) -> String {
    format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">{entries}</DIDL-Lite>"
    )
}

/// Action name from the SOAPACTION header:
/// `"urn:schemas-upnp-org:service:ContentDirectory:1#Browse"` → `Browse`.
fn soap_action(req: &HttpRequest) -> Option<String> {
    let raw = req.headers().get("SOAPACTION")?.to_str().ok()?;
    let trimmed = raw.trim_matches('"');
    Some(trimmed.rsplit('#').next()?.to_string())
}

/// Extract `<Name>value</Name>` (optionally namespace-prefixed) from a SOAP
/// body without a full XML parser.
fn soap_arg(body: &str, name: &str) -> Option<String> {
    let bytes = body.as_bytes();
    let mut search_from = 0;
    while let Some(pos) = body[search_from..].find(name) {
        let abs = search_from + pos;
        search_from = abs + name.len();
        let before = abs.checked_sub(1).map(|i| bytes[i] as char);
        if before != Some('<') && before != Some(':') {
            continue;
        }
        let after_idx = abs + name.len();
        let after = bytes.get(after_idx).map(|&b| b as char);
        if !matches!(after, Some('>') | Some(' ') | Some('/')) {
            continue;
        }
        let rest = &body[after_idx..];
        let gt = rest.find('>')?;
        if rest[..gt].ends_with('/') {
            return Some(String::new());
        }
        let content = &rest[gt + 1..];
        let end = content.find("</")?;
        return Some(xml_unescape(&content[..end]));
    }
    None
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn soap_ok(service_type: &str, action: &str, inner: String) -> HttpResponse {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body><u:{action}Response xmlns:u=\"{service_type}\">{inner}</u:{action}Response></s:Body>\
         </s:Envelope>"
    );
    HttpResponse::Ok()
        .content_type("text/xml; charset=\"utf-8\"")
        .insert_header(("Server", SERVER_HEADER))
        .insert_header(("EXT", ""))
        .body(body)
}

fn soap_fault(code: u32, description: &str) -> HttpResponse {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>\
         <detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\">\
         <errorCode>{code}</errorCode><errorDescription>{description}</errorDescription>\
         </UPnPError></detail></s:Fault></s:Body></s:Envelope>"
    );
    HttpResponse::InternalServerError()
        .content_type("text/xml; charset=\"utf-8\"")
        .insert_header(("Server", SERVER_HEADER))
        .insert_header(("EXT", ""))
        .body(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soap_arg_extracts_plain_and_prefixed_tags() {
        let body = r#"<u:Browse xmlns:u="urn:x"><ObjectID>album:al-1</ObjectID>
            <BrowseFlag>BrowseDirectChildren</BrowseFlag>
            <StartingIndex>0</StartingIndex><RequestedCount>25</RequestedCount></u:Browse>"#;
        assert_eq!(soap_arg(body, "ObjectID").as_deref(), Some("album:al-1"));
        assert_eq!(
            soap_arg(body, "BrowseFlag").as_deref(),
            Some("BrowseDirectChildren")
        );
        assert_eq!(soap_arg(body, "RequestedCount").as_deref(), Some("25"));
        assert_eq!(soap_arg(body, "Missing"), None);
    }

    #[test]
    fn soap_arg_unescapes_entities() {
        let body = "<ObjectID>a&amp;b &lt;c&gt;</ObjectID>";
        assert_eq!(soap_arg(body, "ObjectID").as_deref(), Some("a&b <c>"));
    }

    #[test]
    fn soap_arg_handles_empty_and_self_closing() {
        assert_eq!(soap_arg("<Filter></Filter>", "Filter").as_deref(), Some(""));
        assert_eq!(soap_arg("<Filter/>", "Filter").as_deref(), Some(""));
    }

    #[test]
    fn hms_formats_duration() {
        assert_eq!(hms(0), "0:00:00");
        assert_eq!(hms(61_000), "0:01:01");
        assert_eq!(hms(3_723_000), "1:02:03");
    }

    #[test]
    fn page_clamps_out_of_range() {
        let items = [1, 2, 3, 4, 5];
        assert_eq!(page(&items, 0, 0), &items[..]);
        assert_eq!(page(&items, 2, 2), &[3, 4]);
        assert_eq!(page(&items, 4, 10), &[5]);
        assert_eq!(page(&items, 10, 5), &[] as &[i32]);
    }
}
