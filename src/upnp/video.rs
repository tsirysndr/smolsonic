use super::UpnpState;
use crate::server::repo;
use actix_web::{
    http::{header, Method, StatusCode},
    web, HttpRequest, HttpResponse,
};
use std::{io, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub(super) fn content_type(container: &str) -> &'static str {
    match container {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

pub(super) async fn stream(
    state: web::Data<Arc<UpnpState>>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    let raw = path.into_inner();
    let id = raw.split('.').next().unwrap_or(&raw);
    match repo::find_video(&state.pool, id).await {
        Ok(Some(video)) => {
            serve_file(
                &video.path,
                content_type(&video.container),
                &req,
                "Streaming",
            )
            .await
        }
        Ok(None) => HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!("upnp video lookup {id}: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

pub(super) async fn poster(
    state: web::Data<Arc<UpnpState>>,
    path: web::Path<String>,
    req: HttpRequest,
) -> HttpResponse {
    match repo::find_video(&state.pool, &path.into_inner()).await {
        Ok(Some(video)) => match video.poster_path {
            Some(path) => {
                let mime = mime_guess::from_path(&path)
                    .first_or_octet_stream()
                    .to_string();
                serve_file(&path, &mime, &req, "Interactive").await
            }
            None => HttpResponse::NotFound().finish(),
        },
        Ok(None) => HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!("upnp video poster lookup: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

// A single byte range, including open-ended and suffix requests. Multi-range
// requests are rejected instead of allocating a multipart response in memory.
fn byte_range(value: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let (start, end) = value.strip_prefix("bytes=")?.trim().split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        return (suffix > 0).then(|| (size.saturating_sub(suffix), size - 1));
    }
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().ok()?.min(size - 1)
    };
    (start <= end && start < size).then_some((start, end))
}

async fn serve_file(path: &str, mime: &str, req: &HttpRequest, mode: &str) -> HttpResponse {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => return HttpResponse::NotFound().finish(),
    };
    let size = match file.metadata().await {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => return HttpResponse::NotFound().finish(),
    };
    let mut start = 0;
    let mut length = size;
    let mut response = HttpResponse::Ok();
    if req.method() != Method::HEAD {
        if let Some(range) = req.headers().get(header::RANGE) {
            let Some((from, to)) = range.to_str().ok().and_then(|r| byte_range(r, size)) else {
                return HttpResponse::RangeNotSatisfiable()
                    .insert_header((header::CONTENT_RANGE, format!("bytes */{size}")))
                    .finish();
            };
            start = from;
            length = to - from + 1;
            response
                .status(StatusCode::PARTIAL_CONTENT)
                .insert_header((header::CONTENT_RANGE, format!("bytes {from}-{to}/{size}")));
        }
    }
    response
        .content_type(mime.to_owned())
        .insert_header((header::CONTENT_LENGTH, length.to_string()))
        .insert_header((header::ACCEPT_RANGES, "bytes"))
        .insert_header(("transferMode.dlna.org", mode.to_owned()))
        .insert_header(("contentFeatures.dlna.org", "DLNA.ORG_OP=01;DLNA.ORG_CI=0"));
    if req.method() == Method::HEAD {
        return response.finish();
    }
    if file.seek(io::SeekFrom::Start(start)).await.is_err() {
        return HttpResponse::InternalServerError().finish();
    }
    // Backpressure keeps even multi-gigabyte files at one 64 KiB chunk per stream.
    let chunks = futures::stream::try_unfold((file, length), |(mut file, left)| async move {
        if left == 0 {
            return Ok::<_, io::Error>(None);
        }
        let mut buffer = vec![0; left.min(64 * 1024) as usize];
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Media file truncated",
            ));
        }
        buffer.truncate(read);
        Ok(Some((web::Bytes::from(buffer), (file, left - read as u64))))
    });
    response.streaming(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};

    fn browse_request(id: &str, flag: &str, start: usize, count: usize) -> test::TestRequest {
        test::TestRequest::post().uri("/ctl/ContentDirectory")
            .insert_header(("SOAPAction", "\"urn:schemas-upnp-org:service:ContentDirectory:1#Browse\""))
            .set_payload(format!("<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:Browse xmlns:u=\"urn:schemas-upnp-org:service:ContentDirectory:1\"><ObjectID>{id}</ObjectID><BrowseFlag>{flag}</BrowseFlag><StartingIndex>{start}</StartingIndex><RequestedCount>{count}</RequestedCount></u:Browse></s:Body></s:Envelope>"))
    }

    #[actix_rt::test]
    async fn videos_are_browsable_and_streamable_over_upnp() {
        let dir = std::env::temp_dir().join(format!(
            "smolsonic-upnp-video-{}",
            crate::jellyfin::auth::random_hex(8)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let media = dir.join("movie.mp4");
        let poster = dir.join("poster.jpg");
        let bytes: Vec<u8> = (0..150_000).map(|n| (n % 251) as u8).collect();
        std::fs::write(&media, &bytes).unwrap();
        std::fs::write(&poster, b"poster").unwrap();
        let pool = crate::db::init(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        for id in ["v2", "v1", "v3"] {
            sqlx::query("INSERT INTO videos (id, path, title, container, duration_ms, filesize, bitrate, width, height, poster_path, mtime)
                VALUES (?1, ?2, 'A & B', 'mp4', 61000, 150000, 800000, 1920, 1080, ?3, 0)")
                .bind(id).bind(if id == "v1" { media.clone() } else { dir.join(format!("{id}.mp4")) }.to_string_lossy().to_string())
                .bind(if id == "v1" { Some(poster.to_string_lossy().to_string()) } else { None })
                .execute(&pool).await.unwrap();
        }
        let state = web::Data::new(Arc::new(UpnpState {
            pool: pool.clone(),
            covers_dir: dir.clone(),
            friendly_name: "Test".into(),
            uuid: "test".into(),
            subsonic_port: 4533,
        }));
        let app = test::init_service(
            App::new()
                .app_data(state)
                .configure(super::super::configure_routes),
        )
        .await;
        let response = test::call_service(
            &app,
            browse_request("0", "BrowseDirectChildren", 0, 0).to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
        assert!(body.contains("&lt;dc:title&gt;Videos&lt;/dc:title&gt;"));
        assert!(body.contains("<TotalMatches>5</TotalMatches>"));
        for (start, count, returned) in [
            (0, 1, 1),
            (1, 1, 1),
            (2, 10, 1),
            (0, 0, 3),
            (1, 0, 2),
            (99, 2, 0),
        ] {
            let response = test::call_service(
                &app,
                browse_request("videos", "BrowseDirectChildren", start, count).to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
            assert!(body.contains("<TotalMatches>3</TotalMatches>"));
            assert!(body.contains(&format!("<NumberReturned>{returned}</NumberReturned>")));
            if start < 3 {
                assert!(body.contains(&format!("video:v{}", start + 1)));
            }
        }
        for id in ["videos", "video:v1"] {
            let response = test::call_service(
                &app,
                browse_request(id, "BrowseMetadata", 0, 1).to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
            if id == "video:v1" {
                assert!(body.contains("object.item.videoItem.movie"));
                assert!(body.contains("A &amp;amp; B"));
                assert!(body.contains("/video/v1.mp4"));
                assert!(body.contains("/video-art/v1"));
                assert!(body.contains("video/mp4"));
                assert!(body.contains("resolution=&quot;1920x1080&quot;"));
                assert!(body.contains("bitrate=&quot;100000&quot;"));
                assert!(body.contains("duration=&quot;0:01:01&quot;"));
            }
        }
        let response = test::call_service(
            &app,
            browse_request("video:missing", "BrowseMetadata", 0, 1).to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
        assert!(body.contains("<errorCode>701</errorCode>"));
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/ctl/ConnectionManager")
                .insert_header((
                    "SOAPAction",
                    "\"urn:schemas-upnp-org:service:ConnectionManager:1#GetProtocolInfo\"",
                ))
                .to_request(),
        )
        .await;
        let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
        for mime in [
            "video/mp4",
            "video/x-matroska",
            "video/webm",
            "video/quicktime",
            "video/x-msvideo",
        ] {
            assert!(body.contains(&format!("http-get:*:{mime}:*")));
        }
        let response = test::call_service(
            &app,
            test::TestRequest::get().uri("/video/v1.mp4").to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "video/mp4"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_LENGTH).unwrap(),
            "150000"
        );
        assert_eq!(
            response.headers().get("transferMode.dlna.org").unwrap(),
            "Streaming"
        );
        assert_eq!(test::read_body(response).await.as_ref(), &bytes);
        for (range, start, end) in [
            ("bytes=65530-65550", 65530, 65550),
            ("bytes=149990-", 149990, 149999),
            ("bytes=-5", 149995, 149999),
            ("bytes=149998-999999", 149998, 149999),
        ] {
            let response = test::call_service(
                &app,
                test::TestRequest::get()
                    .uri("/video/v1.mp4")
                    .insert_header((header::RANGE, range))
                    .to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                format!("bytes {start}-{end}/150000")
            );
            assert_eq!(
                test::read_body(response).await.as_ref(),
                &bytes[start..=end]
            );
        }
        for range in [
            "bytes=150000-",
            "bytes=8-2",
            "bytes=-0",
            "bytes=0-1,3-4",
            "bytes=bad",
        ] {
            let response = test::call_service(
                &app,
                test::TestRequest::get()
                    .uri("/video/v1.mp4")
                    .insert_header((header::RANGE, range))
                    .to_request(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
            assert_eq!(
                response.headers().get(header::CONTENT_RANGE).unwrap(),
                "bytes */150000"
            );
        }
        let response = test::call_service(
            &app,
            test::TestRequest::default()
                .method(Method::HEAD)
                .uri("/video/v1.mp4")
                .insert_header((header::RANGE, "bytes=10-20"))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_LENGTH).unwrap(),
            "150000"
        );
        assert!(test::read_body(response).await.is_empty());
        let response = test::call_service(
            &app,
            test::TestRequest::get().uri("/video-art/v1").to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        assert_eq!(test::read_body(response).await.as_ref(), b"poster");
        for uri in [
            "/video/v2.mp4",
            "/video/missing.mp4",
            "/video-art/v2",
            "/video-art/missing",
        ] {
            let response =
                test::call_service(&app, test::TestRequest::get().uri(uri).to_request()).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        sqlx::query("DELETE FROM videos")
            .execute(&pool)
            .await
            .unwrap();
        let response = test::call_service(
            &app,
            browse_request("videos", "BrowseDirectChildren", 0, 0).to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
        assert!(body.contains("<TotalMatches>0</TotalMatches>"));
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
