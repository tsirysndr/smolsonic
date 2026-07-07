//! UPnP AV MediaServer:1 ("DLNA server"). Three moving parts:
//!
//!  * SSDP (ssdp.rs) — answers M-SEARCH probes on udp/1900 and multicasts
//!    periodic `NOTIFY ssdp:alive` so renderers find us without a probe.
//!  * Description — `GET /rootDesc.xml` plus the two SCPD documents that
//!    declare which SOAP actions we implement.
//!  * Control — `POST /ctl/ContentDirectory` (Browse over the library) and
//!    `POST /ctl/ConnectionManager` (protocol-info boilerplate), answered in
//!    content_directory.rs.
//!
//! Media itself is served from this same HTTP port (`/stream/{id}`,
//! `/art/{id}`) with Range support and no auth — DLNA has no auth concept,
//! which is why the whole feature is opt-in via the `[upnp]` config block.

pub mod content_directory;
pub mod ssdp;

use crate::config::UpnpConfig;
use crate::db::Db;
use crate::server::repo;
use actix_web::http::Method;
use actix_web::{guard, web, App, HttpRequest, HttpResponse, HttpServer};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

pub const DEVICE_TYPE: &str = "urn:schemas-upnp-org:device:MediaServer:1";
pub const CD_SERVICE_TYPE: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";
pub const CM_SERVICE_TYPE: &str = "urn:schemas-upnp-org:service:ConnectionManager:1";
pub const SERVER_HEADER: &str = concat!(
    "smolsonic/",
    env!("CARGO_PKG_VERSION"),
    " UPnP/1.0 DLNADOC/1.50"
);

pub struct UpnpState {
    pub pool: Db,
    pub covers_dir: PathBuf,
    pub friendly_name: String,
    pub uuid: String,
    /// Port of the main Subsonic server, used as presentationURL.
    pub subsonic_port: u16,
}

/// Stable device UUID, minted once and persisted so control points that key
/// on the UDN keep recognizing us across restarts.
pub async fn ensure_device_uuid(pool: &Db) -> Result<String> {
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT value FROM upnp_meta WHERE key = 'device_uuid'")
            .fetch_optional(pool)
            .await?;
    if let Some((v,)) = existing {
        return Ok(v);
    }
    let id = crate::jellyfin::mapping::guid_dashed(&crate::jellyfin::auth::random_hex(16));
    sqlx::query(
        "INSERT INTO upnp_meta (key, value) VALUES ('device_uuid', ?1)
         ON CONFLICT(key) DO NOTHING",
    )
    .bind(&id)
    .execute(pool)
    .await?;
    let (val,): (String,) = sqlx::query_as("SELECT value FROM upnp_meta WHERE key = 'device_uuid'")
        .fetch_one(pool)
        .await?;
    Ok(val)
}

pub async fn start(
    cfg: UpnpConfig,
    pool: Db,
    covers_dir: PathBuf,
    uuid: String,
    subsonic_port: u16,
) -> Result<()> {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let state = web::Data::new(Arc::new(UpnpState {
        pool,
        covers_dir,
        friendly_name: cfg.friendly_name,
        uuid,
        subsonic_port,
    }));

    tracing::info!("starting UPnP/DLNA media server on {addr}");

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .configure(configure_routes)
    })
    .bind(&addr)?
    .run()
    .await?;
    Ok(())
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    let subscribe = || Method::from_bytes(b"SUBSCRIBE").expect("valid method");
    let unsubscribe = || Method::from_bytes(b"UNSUBSCRIBE").expect("valid method");
    cfg.route("/rootDesc.xml", web::get().to(root_desc))
        .route("/ContentDirectory.xml", web::get().to(cd_scpd))
        .route("/ConnectionManager.xml", web::get().to(cm_scpd))
        .route(
            "/ctl/ContentDirectory",
            web::post().to(content_directory::control),
        )
        .route(
            "/ctl/ConnectionManager",
            web::post().to(content_directory::connection_manager_control),
        )
        .route(
            "/evt/{service}",
            web::route().method(subscribe()).to(subscribe_stub),
        )
        .route(
            "/evt/{service}",
            web::route().method(unsubscribe()).to(unsubscribe_stub),
        )
        .route(
            "/stream/{id}",
            web::route()
                .guard(guard::Any(guard::Get()).or(guard::Head()))
                .to(stream),
        )
        .route(
            "/art/{id}",
            web::route()
                .guard(guard::Any(guard::Get()).or(guard::Head()))
                .to(album_art),
        );
}

pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// `http://<host the client dialed>` — building URLs from the Host header
/// keeps them correct on multi-homed machines, since the client already
/// reached us on a routable address.
fn base_url(req: &HttpRequest) -> String {
    format!("http://{}", req.connection_info().host())
}

async fn root_desc(state: web::Data<Arc<UpnpState>>, req: HttpRequest) -> HttpResponse {
    let base = base_url(&req);
    let name = xml_escape(&state.friendly_name);
    let uuid = &state.uuid;
    let presentation = {
        let host = req.connection_info().host().to_string();
        let host_only = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(&host);
        format!("http://{}:{}/", host_only, state.subsonic_port)
    };
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0" xmlns:dlna="urn:schemas-dlna-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>{device_type}</deviceType>
    <friendlyName>{name}</friendlyName>
    <manufacturer>smolsonic</manufacturer>
    <manufacturerURL>https://github.com/tsirysndr/smolsonic</manufacturerURL>
    <modelName>smolsonic</modelName>
    <modelDescription>A tiny Subsonic-compatible music server</modelDescription>
    <modelNumber>{version}</modelNumber>
    <modelURL>https://github.com/tsirysndr/smolsonic</modelURL>
    <UDN>uuid:{uuid}</UDN>
    <dlna:X_DLNADOC xmlns:dlna="urn:schemas-dlna-org:device-1-0">DMS-1.50</dlna:X_DLNADOC>
    <presentationURL>{presentation}</presentationURL>
    <serviceList>
      <service>
        <serviceType>{cd_type}</serviceType>
        <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>
        <SCPDURL>{base}/ContentDirectory.xml</SCPDURL>
        <controlURL>{base}/ctl/ContentDirectory</controlURL>
        <eventSubURL>{base}/evt/ContentDirectory</eventSubURL>
      </service>
      <service>
        <serviceType>{cm_type}</serviceType>
        <serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>
        <SCPDURL>{base}/ConnectionManager.xml</SCPDURL>
        <controlURL>{base}/ctl/ConnectionManager</controlURL>
        <eventSubURL>{base}/evt/ConnectionManager</eventSubURL>
      </service>
    </serviceList>
  </device>
</root>"#,
        device_type = DEVICE_TYPE,
        cd_type = CD_SERVICE_TYPE,
        cm_type = CM_SERVICE_TYPE,
        version = env!("CARGO_PKG_VERSION"),
    );
    xml_response(body)
}

fn xml_response(body: String) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/xml; charset=\"utf-8\"")
        .insert_header(("Server", SERVER_HEADER))
        .body(body)
}

async fn cd_scpd() -> HttpResponse {
    xml_response(CONTENT_DIRECTORY_SCPD.to_string())
}

async fn cm_scpd() -> HttpResponse {
    xml_response(CONNECTION_MANAGER_SCPD.to_string())
}

/// GENA stub: hand out a subscription id and never send events. Renderers
/// subscribe as a matter of course; failing the request makes some of them
/// drop the server, while a silent subscription is universally tolerated.
async fn subscribe_stub() -> HttpResponse {
    let sid = format!("uuid:{}", crate::jellyfin::auth::random_hex(16));
    HttpResponse::Ok()
        .insert_header(("SID", sid))
        .insert_header(("TIMEOUT", "Second-1800"))
        .insert_header(("Server", SERVER_HEADER))
        .finish()
}

async fn unsubscribe_stub() -> HttpResponse {
    HttpResponse::Ok()
        .insert_header(("Server", SERVER_HEADER))
        .finish()
}

async fn stream(
    state: web::Data<Arc<UpnpState>>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    // Accept "so-…" and "so-….mp3" — some renderers only play URLs that end
    // in a recognizable extension, so browse results append the suffix.
    let raw = path.into_inner();
    let id = raw.split('.').next().unwrap_or(&raw).to_string();
    let song = match repo::find_song(&state.pool, &id).await {
        Ok(Some(s)) => s,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!("upnp stream lookup {id}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let path = PathBuf::from(&song.path);
    let file_size = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::error!("upnp stream stat {}: {e}", song.path);
            return HttpResponse::NotFound().finish();
        }
    };

    let dlna_headers = [
        ("transferMode.dlna.org", "Streaming".to_string()),
        (
            "contentFeatures.dlna.org",
            "DLNA.ORG_OP=01;DLNA.ORG_CI=0".to_string(),
        ),
    ];

    if let Some(range_hdr) = req.headers().get(actix_web::http::header::RANGE) {
        if let Ok(range_str) = range_hdr.to_str() {
            if let Some(range) = range_str.strip_prefix("bytes=") {
                let mut parts = range.splitn(2, '-');
                let start: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let end: u64 = parts
                    .next()
                    .filter(|s| !s.is_empty())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(file_size.saturating_sub(1))
                    .min(file_size.saturating_sub(1));
                if start <= end && file_size > 0 {
                    use std::io::{Read, Seek, SeekFrom};
                    return match std::fs::File::open(&path) {
                        Ok(mut file) => {
                            let _ = file.seek(SeekFrom::Start(start));
                            let length = (end - start + 1) as usize;
                            let mut buf = vec![0u8; length];
                            let n = file.read(&mut buf).unwrap_or(0);
                            buf.truncate(n);
                            let actual_end = start + n as u64 - 1;
                            let mut resp = HttpResponse::PartialContent();
                            resp.content_type(song.content_type.clone())
                                .insert_header(("Accept-Ranges", "bytes"))
                                .insert_header(("Content-Length", n.to_string()))
                                .insert_header((
                                    "Content-Range",
                                    format!("bytes {}-{}/{}", start, actual_end, file_size),
                                ));
                            for (k, v) in &dlna_headers {
                                resp.insert_header((*k, v.clone()));
                            }
                            resp.body(buf)
                        }
                        Err(e) => {
                            tracing::error!("upnp stream open {}: {e}", song.path);
                            HttpResponse::InternalServerError().finish()
                        }
                    };
                }
            }
        }
    }

    match std::fs::read(&path) {
        Ok(data) => {
            let mut resp = HttpResponse::Ok();
            resp.content_type(song.content_type)
                .insert_header(("Accept-Ranges", "bytes"))
                .insert_header(("Content-Length", file_size.to_string()));
            for (k, v) in &dlna_headers {
                resp.insert_header((*k, v.clone()));
            }
            resp.body(data)
        }
        Err(e) => {
            tracing::error!("upnp stream read {}: {e}", song.path);
            HttpResponse::InternalServerError().finish()
        }
    }
}

async fn album_art(state: web::Data<Arc<UpnpState>>, path: web::Path<String>) -> HttpResponse {
    let raw = path.into_inner();
    let id = raw.split('.').next().unwrap_or(&raw).to_string();
    let cover = match repo::find_album(&state.pool, &id).await {
        Ok(Some(a)) => a.cover_art,
        Ok(None) => None,
        Err(e) => {
            tracing::error!("upnp art lookup {id}: {e}");
            None
        }
    };
    let Some(filename) = cover else {
        return HttpResponse::NotFound().finish();
    };
    let full = state.covers_dir.join(&filename);
    match std::fs::read(&full) {
        Ok(data) => {
            let mime = mime_guess::from_path(&full)
                .first_or_octet_stream()
                .to_string();
            HttpResponse::Ok()
                .content_type(mime)
                .insert_header(("transferMode.dlna.org", "Interactive"))
                .body(data)
        }
        Err(e) => {
            tracing::warn!("upnp art read {}: {e}", full.display());
            HttpResponse::NotFound().finish()
        }
    }
}

const CONTENT_DIRECTORY_SCPD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action>
      <name>Browse</name>
      <argumentList>
        <argument><name>ObjectID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ObjectID</relatedStateVariable></argument>
        <argument><name>BrowseFlag</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_BrowseFlag</relatedStateVariable></argument>
        <argument><name>Filter</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Filter</relatedStateVariable></argument>
        <argument><name>StartingIndex</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Index</relatedStateVariable></argument>
        <argument><name>RequestedCount</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>SortCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SortCriteria</relatedStateVariable></argument>
        <argument><name>Result</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Result</relatedStateVariable></argument>
        <argument><name>NumberReturned</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>TotalMatches</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>UpdateID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_UpdateID</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetSearchCapabilities</name>
      <argumentList>
        <argument><name>SearchCaps</name><direction>out</direction><relatedStateVariable>SearchCapabilities</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetSortCapabilities</name>
      <argumentList>
        <argument><name>SortCaps</name><direction>out</direction><relatedStateVariable>SortCapabilities</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetSystemUpdateID</name>
      <argumentList>
        <argument><name>Id</name><direction>out</direction><relatedStateVariable>SystemUpdateID</relatedStateVariable></argument>
      </argumentList>
    </action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ObjectID</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Result</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_BrowseFlag</name><dataType>string</dataType>
      <allowedValueList><allowedValue>BrowseMetadata</allowedValue><allowedValue>BrowseDirectChildren</allowedValue></allowedValueList>
    </stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Filter</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_SortCriteria</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Index</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Count</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_UpdateID</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SearchCapabilities</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SortCapabilities</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>SystemUpdateID</name><dataType>ui4</dataType></stateVariable>
  </serviceStateTable>
</scpd>"#;

const CONNECTION_MANAGER_SCPD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action>
      <name>GetProtocolInfo</name>
      <argumentList>
        <argument><name>Source</name><direction>out</direction><relatedStateVariable>SourceProtocolInfo</relatedStateVariable></argument>
        <argument><name>Sink</name><direction>out</direction><relatedStateVariable>SinkProtocolInfo</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetCurrentConnectionIDs</name>
      <argumentList>
        <argument><name>ConnectionIDs</name><direction>out</direction><relatedStateVariable>CurrentConnectionIDs</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetCurrentConnectionInfo</name>
      <argumentList>
        <argument><name>ConnectionID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
        <argument><name>RcsID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_RcsID</relatedStateVariable></argument>
        <argument><name>AVTransportID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_AVTransportID</relatedStateVariable></argument>
        <argument><name>ProtocolInfo</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ProtocolInfo</relatedStateVariable></argument>
        <argument><name>PeerConnectionManager</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionManager</relatedStateVariable></argument>
        <argument><name>PeerConnectionID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
        <argument><name>Direction</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Direction</relatedStateVariable></argument>
        <argument><name>Status</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionStatus</relatedStateVariable></argument>
      </argumentList>
    </action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="yes"><name>SourceProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>SinkProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>CurrentConnectionIDs</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionStatus</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionManager</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Direction</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionID</name><dataType>i4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_AVTransportID</name><dataType>i4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_RcsID</name><dataType>i4</dataType></stateVariable>
  </serviceStateTable>
</scpd>"#;
