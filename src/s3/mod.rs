pub mod admin;
pub mod handlers;
pub mod sigv4;

use crate::config::S3Config;
use actix_web::{web, App, HttpServer};
use std::path::PathBuf;
use std::sync::Arc;

pub const BUCKET: &str = "music";
pub const REGION: &str = "us-east-1";
pub const SERVICE: &str = "s3";

#[cfg(target_pointer_width = "64")]
const MAX_UPLOAD: usize = 8 * 1024 * 1024 * 1024;
#[cfg(not(target_pointer_width = "64"))]
const MAX_UPLOAD: usize = 1024 * 1024 * 1024;

pub struct S3State {
    pub music_dir: PathBuf,
    /// Optional video library root. When set, S3 PUTs with a video extension
    /// (mkv/mp4/webm/mov/avi/m4v) land here instead of `music_dir`, and the
    /// LIST/GET/HEAD/DELETE handlers consult both directories.
    pub video_dir: Option<PathBuf>,
    pub access_key: Arc<String>,
    pub secret_key: Arc<String>,
}

pub async fn start(
    cfg: S3Config,
    music_dir: PathBuf,
    video_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let state = web::Data::new(S3State {
        music_dir,
        video_dir,
        access_key: Arc::new(cfg.access_key),
        secret_key: Arc::new(cfg.secret_key),
    });

    tracing::info!(
        "starting S3 API on {addr} (bucket={BUCKET}, region={REGION}, admin UI at /admin)"
    );

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(web::PayloadConfig::new(MAX_UPLOAD))
            .configure(admin::configure)
            .configure(configure_routes)
    })
    .bind(&addr)?
    .run()
    .await?;
    Ok(())
}

/// S3 routes (no admin UI). Extracted so tests can mount them on an
/// `App::configure(configure_routes)` against an in-memory state.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/", web::get().to(handlers::list_buckets))
        .route("/{bucket}", web::get().to(handlers::list_objects))
        .route("/{bucket}/", web::get().to(handlers::list_objects))
        .route("/{bucket}", web::head().to(handlers::head_bucket))
        .route("/{bucket}/", web::head().to(handlers::head_bucket))
        .route("/{bucket}/{key:.*}", web::get().to(handlers::get_object))
        .route("/{bucket}/{key:.*}", web::head().to(handlers::head_object))
        .route("/{bucket}/{key:.*}", web::put().to(handlers::put_object))
        .route(
            "/{bucket}/{key:.*}",
            web::delete().to(handlers::delete_object),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test, App};
    use sha2::{Digest, Sha256};

    const ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const HOST: &str = "127.0.0.1:9000";
    const AMZ_DATE: &str = "20200101T000000Z";
    const SCOPE_DATE: &str = "20200101";

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("smolsonic-s3-{tag}-{pid}-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn build_state(music_dir: std::path::PathBuf) -> web::Data<S3State> {
        web::Data::new(S3State {
            music_dir,
            video_dir: None,
            access_key: Arc::new(ACCESS_KEY.to_string()),
            secret_key: Arc::new(SECRET_KEY.to_string()),
        })
    }

    fn build_state_with_video(
        music_dir: std::path::PathBuf,
        video_dir: std::path::PathBuf,
    ) -> web::Data<S3State> {
        web::Data::new(S3State {
            music_dir,
            video_dir: Some(video_dir),
            access_key: Arc::new(ACCESS_KEY.to_string()),
            secret_key: Arc::new(SECRET_KEY.to_string()),
        })
    }

    /// Build a signed TestRequest. The `query` must be in canonical form
    /// (already uri-encoded and sorted) so signing and routing agree.
    fn signed(
        method: actix_web::http::Method,
        path: &str,
        query: &str,
        body_sha: &str,
    ) -> test::TestRequest {
        let full_path = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        let headers: Vec<(&str, &str)> = vec![
            ("host", HOST),
            ("x-amz-date", AMZ_DATE),
            ("x-amz-content-sha256", body_sha),
        ];
        let signed_headers = ["host", "x-amz-content-sha256", "x-amz-date"];
        let auth = sigv4::sign_authorization(
            method.as_str(),
            path,
            query,
            &headers,
            &signed_headers,
            body_sha,
            ACCESS_KEY,
            SECRET_KEY,
            REGION,
            SERVICE,
            AMZ_DATE,
            SCOPE_DATE,
        );
        test::TestRequest::default()
            .method(method)
            .uri(&full_path)
            .insert_header(("host", HOST))
            .insert_header(("x-amz-date", AMZ_DATE))
            .insert_header(("x-amz-content-sha256", body_sha))
            .insert_header(("authorization", auth))
    }

    #[actix_web::test]
    async fn list_buckets_requires_valid_signature() {
        let dir = tempdir("list_buckets");
        let app = test::init_service(
            App::new().app_data(build_state(dir.clone())).configure(configure_routes),
        )
        .await;

        // No auth headers at all → looks like a browser, returns the help page (200 text/plain),
        // not an S3 client. The S3 SDK heuristic in handlers::list_buckets only kicks in when
        // SigV4 markers are present. So we trigger an S3-style request, but with a *bad* signature.
        let bad = test::TestRequest::get()
            .uri("/")
            .insert_header(("host", HOST))
            .insert_header(("x-amz-date", AMZ_DATE))
            .insert_header(("x-amz-content-sha256", sigv4::EMPTY_BODY_SHA256))
            .insert_header((
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/{SCOPE_DATE}/{REGION}/{SERVICE}/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=deadbeef"
                ),
            ))
            .to_request();
        let resp = test::call_service(&app, bad).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Properly signed → XML listing the single `music` bucket.
        let good = signed(
            actix_web::http::Method::GET,
            "/",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, good).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let xml = std::str::from_utf8(&body).unwrap();
        assert!(xml.contains("<Name>music</Name>"));
    }

    #[actix_web::test]
    async fn head_bucket_unknown_returns_404_without_auth_check() {
        let dir = tempdir("head_bucket");
        let app = test::init_service(
            App::new().app_data(build_state(dir.clone())).configure(configure_routes),
        )
        .await;

        // Unknown bucket → 404 short-circuit (bucket check runs before signature check).
        let req = test::TestRequest::default()
            .method(actix_web::http::Method::HEAD)
            .uri("/notmusic")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Correct bucket + valid signature → 200.
        let req = signed(
            actix_web::http::Method::HEAD,
            "/music",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn put_get_head_delete_roundtrip() {
        let dir = tempdir("rt");
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .app_data(web::PayloadConfig::new(MAX_UPLOAD))
                .configure(configure_routes),
        )
        .await;

        let payload = b"hello smolsonic".to_vec();
        let payload_sha = hex::encode(Sha256::digest(&payload));

        // PUT /music/Artist/Album/song.mp3
        let req = signed(
            actix_web::http::Method::PUT,
            "/music/Artist/Album/song.mp3",
            "",
            &payload_sha,
        )
        .set_payload(payload.clone())
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let etag = resp
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        // ETag is the md5 of the body (S3-style).
        let expected_md5 = hex::encode(md5::Md5::digest(&payload));
        assert_eq!(etag, format!("\"{expected_md5}\""));

        // File landed under music_dir.
        let on_disk = dir.join("Artist").join("Album").join("song.mp3");
        assert!(on_disk.exists(), "expected file at {}", on_disk.display());
        assert_eq!(std::fs::read(&on_disk).unwrap(), payload);

        // HEAD reports size + audio/mpeg.
        let req = signed(
            actix_web::http::Method::HEAD,
            "/music/Artist/Album/song.mp3",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-length").unwrap().to_str().unwrap(),
            &payload.len().to_string()
        );
        assert_eq!(
            resp.headers().get("content-type").unwrap().to_str().unwrap(),
            "audio/mpeg"
        );

        // GET returns the same bytes.
        let req = signed(
            actix_web::http::Method::GET,
            "/music/Artist/Album/song.mp3",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = test::read_body(resp).await;
        assert_eq!(bytes.as_ref(), payload.as_slice());

        // DELETE.
        let req = signed(
            actix_web::http::Method::DELETE,
            "/music/Artist/Album/song.mp3",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!on_disk.exists());

        // GET on the now-missing key → 404 NoSuchKey.
        let req = signed(
            actix_web::http::Method::GET,
            "/music/Artist/Album/song.mp3",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = test::read_body(resp).await;
        let xml = std::str::from_utf8(&body).unwrap();
        assert!(xml.contains("<Code>NoSuchKey</Code>"));
    }

    #[actix_web::test]
    async fn put_with_unsigned_payload_marker_is_accepted() {
        let dir = tempdir("unsigned");
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .app_data(web::PayloadConfig::new(MAX_UPLOAD))
                .configure(configure_routes),
        )
        .await;

        let payload = b"raw bytes, no body hash".to_vec();
        // x-amz-content-sha256: UNSIGNED-PAYLOAD is signed *as the literal string*.
        let req = signed(
            actix_web::http::Method::PUT,
            "/music/song.txt",
            "",
            sigv4::UNSIGNED_PAYLOAD,
        )
        .set_payload(payload.clone())
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(std::fs::read(dir.join("song.txt")).unwrap(), payload);
    }

    #[actix_web::test]
    async fn presigned_put_with_signature_in_query_validates() {
        let dir = tempdir("presigned");
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .app_data(web::PayloadConfig::new(MAX_UPLOAD))
                .configure(configure_routes),
        )
        .await;

        // Canonical query for a presigned PUT: alphabetically sorted, percent-encoded,
        // and WITHOUT X-Amz-Signature. The signature is appended only after the
        // canonical query is hashed — including it during verification would
        // always produce a mismatch (the bug this regression test guards against).
        let cred = format!("{ACCESS_KEY}%2F{SCOPE_DATE}%2F{REGION}%2F{SERVICE}%2Faws4_request");
        let canonical_query = format!(
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential={cred}\
             &X-Amz-Date={AMZ_DATE}\
             &X-Amz-Expires=900\
             &X-Amz-SignedHeaders=host"
        );

        let auth = sigv4::sign_authorization(
            "PUT",
            "/music/presigned.bin",
            &canonical_query,
            &[("host", HOST)],
            &["host"],
            sigv4::UNSIGNED_PAYLOAD,
            ACCESS_KEY,
            SECRET_KEY,
            REGION,
            SERVICE,
            AMZ_DATE,
            SCOPE_DATE,
        );
        let signature = auth.rsplit_once("Signature=").unwrap().1.to_string();

        let payload = b"presigned upload".to_vec();
        let uri = format!(
            "/music/presigned.bin?{canonical_query}&X-Amz-Signature={signature}"
        );
        let req = test::TestRequest::put()
            .uri(&uri)
            .insert_header(("host", HOST))
            .insert_header(("x-amz-content-sha256", sigv4::UNSIGNED_PAYLOAD))
            .set_payload(payload.clone())
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(std::fs::read(dir.join("presigned.bin")).unwrap(), payload);
    }

    #[actix_web::test]
    async fn presigned_put_with_content_sha_only_in_query_validates() {
        // Browser AWS SDK v3 sends X-Amz-Content-Sha256 as a query string
        // parameter (not a header) on presigned PUTs. The server must still
        // resolve it to UNSIGNED-PAYLOAD when verifying the signature.
        let dir = tempdir("presigned-querysha");
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .app_data(web::PayloadConfig::new(MAX_UPLOAD))
                .configure(configure_routes),
        )
        .await;

        let cred = format!("{ACCESS_KEY}%2F{SCOPE_DATE}%2F{REGION}%2F{SERVICE}%2Faws4_request");
        let canonical_query = format!(
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Content-Sha256=UNSIGNED-PAYLOAD\
             &X-Amz-Credential={cred}\
             &X-Amz-Date={AMZ_DATE}\
             &X-Amz-Expires=900\
             &X-Amz-SignedHeaders=host"
        );

        let auth = sigv4::sign_authorization(
            "PUT",
            "/music/browser.bin",
            &canonical_query,
            &[("host", HOST)],
            &["host"],
            sigv4::UNSIGNED_PAYLOAD,
            ACCESS_KEY,
            SECRET_KEY,
            REGION,
            SERVICE,
            AMZ_DATE,
            SCOPE_DATE,
        );
        let signature = auth.rsplit_once("Signature=").unwrap().1.to_string();

        let payload = b"browser upload".to_vec();
        let uri = format!(
            "/music/browser.bin?{canonical_query}&X-Amz-Signature={signature}"
        );
        let req = test::TestRequest::put()
            .uri(&uri)
            .insert_header(("host", HOST))
            .set_payload(payload.clone())
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(std::fs::read(dir.join("browser.bin")).unwrap(), payload);
    }

    #[actix_web::test]
    async fn presigned_put_with_special_chars_in_key_validates() {
        // Keys with spaces, brackets, etc. are percent-encoded in the URL. The
        // server must canonicalize by decoding once and re-encoding once;
        // otherwise `%20` becomes `%2520` and the signature never matches.
        let dir = tempdir("presigned-special");
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .app_data(web::PayloadConfig::new(MAX_UPLOAD))
                .configure(configure_routes),
        )
        .await;

        let raw_key = "Audiosoulz - Dancefloor [Music Video]-dZ1EU20GucM.m4v";
        let encoded_key = sigv4::uri_encode(raw_key, false);
        let signing_path = format!("/music/{}", encoded_key);

        let cred = format!("{ACCESS_KEY}%2F{SCOPE_DATE}%2F{REGION}%2F{SERVICE}%2Faws4_request");
        let canonical_query = format!(
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Content-Sha256=UNSIGNED-PAYLOAD\
             &X-Amz-Credential={cred}\
             &X-Amz-Date={AMZ_DATE}\
             &X-Amz-Expires=900\
             &X-Amz-SignedHeaders=host"
        );

        let auth = sigv4::sign_authorization(
            "PUT",
            &format!("/music/{raw_key}"),
            &canonical_query,
            &[("host", HOST)],
            &["host"],
            sigv4::UNSIGNED_PAYLOAD,
            ACCESS_KEY,
            SECRET_KEY,
            REGION,
            SERVICE,
            AMZ_DATE,
            SCOPE_DATE,
        );
        let signature = auth.rsplit_once("Signature=").unwrap().1.to_string();

        let payload = b"video bytes".to_vec();
        let uri = format!("{signing_path}?{canonical_query}&X-Amz-Signature={signature}");
        let req = test::TestRequest::put()
            .uri(&uri)
            .insert_header(("host", HOST))
            .set_payload(payload.clone())
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        // No video_dir configured in build_state → falls back to music_dir.
        assert_eq!(std::fs::read(dir.join(raw_key)).unwrap(), payload);
    }

    #[actix_web::test]
    async fn put_with_wrong_payload_sha_returns_bad_request() {
        let dir = tempdir("badsha");
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .app_data(web::PayloadConfig::new(MAX_UPLOAD))
                .configure(configure_routes),
        )
        .await;

        let payload = b"actual content".to_vec();
        // Claim the sha256 is the empty body's — but send a non-empty body.
        let req = signed(
            actix_web::http::Method::PUT,
            "/music/k.bin",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .set_payload(payload)
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // File must not have been written.
        assert!(!dir.join("k.bin").exists());
    }

    #[actix_web::test]
    async fn list_objects_v2_with_prefix_and_delimiter_groups_common_prefixes() {
        let dir = tempdir("ls");
        // Lay out a small tree:
        //   Artist/Album/track1.mp3
        //   Artist/Album/track2.mp3
        //   Artist/Live/track1.mp3
        //   other.mp3
        for rel in [
            "Artist/Album/track1.mp3",
            "Artist/Album/track2.mp3",
            "Artist/Live/track1.mp3",
            "other.mp3",
        ] {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        }
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .configure(configure_routes),
        )
        .await;

        // ListObjectsV2 with prefix=Artist/ and delimiter=/
        // → expect CommonPrefixes for Artist/Album/ and Artist/Live/.
        let q = "delimiter=%2F&list-type=2&prefix=Artist%2F";
        let req = signed(
            actix_web::http::Method::GET,
            "/music",
            q,
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let xml = std::str::from_utf8(&body).unwrap();
        assert!(xml.contains("<CommonPrefixes><Prefix>Artist/Album/</Prefix></CommonPrefixes>"));
        assert!(xml.contains("<CommonPrefixes><Prefix>Artist/Live/</Prefix></CommonPrefixes>"));
        // No raw <Contents> for the grouped files at this level.
        assert!(!xml.contains("<Key>Artist/Album/track1.mp3</Key>"));

        // Without delimiter, all four files appear as <Contents>.
        let req = signed(
            actix_web::http::Method::GET,
            "/music",
            "list-type=2",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        let body = test::read_body(resp).await;
        let xml = std::str::from_utf8(&body).unwrap();
        for key in [
            "Artist/Album/track1.mp3",
            "Artist/Album/track2.mp3",
            "Artist/Live/track1.mp3",
            "other.mp3",
        ] {
            assert!(xml.contains(&format!("<Key>{key}</Key>")), "missing {key} in {xml}");
        }
    }

    #[actix_web::test]
    async fn invalid_keys_are_rejected_with_bad_request() {
        let dir = tempdir("badkey");
        let app = test::init_service(
            App::new()
                .app_data(build_state(dir.clone()))
                .configure(configure_routes),
        )
        .await;

        // ".." traversal — the catch-all `/{bucket}/{key:.*}` route would match,
        // but the key should resolve to invalid before any filesystem touch.
        // We have to encode the slash in the path so actix routes it as a single
        // segment to {key}; using `/music/a%2F..%2Fb` causes the verify path to
        // see %2F. The handler decodes via PathBuf splits on '/', so encoded
        // segments survive. Easier: send an absolute-looking key.
        let req = signed(
            actix_web::http::Method::GET,
            "/music/%2Fabs",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn video_uploads_route_to_video_dir_audio_to_music_dir() {
        let music_dir = tempdir("split-music");
        let video_dir = tempdir("split-video");
        let app = test::init_service(
            App::new()
                .app_data(build_state_with_video(music_dir.clone(), video_dir.clone()))
                .app_data(web::PayloadConfig::new(MAX_UPLOAD))
                .configure(configure_routes),
        )
        .await;

        // PUT an audio file → lands in music_dir.
        let audio = b"audio payload".to_vec();
        let sha = hex::encode(Sha256::digest(&audio));
        let req = signed(
            actix_web::http::Method::PUT,
            "/music/Artist/Album/track.mp3",
            "",
            &sha,
        )
        .set_payload(audio.clone())
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(music_dir.join("Artist/Album/track.mp3").exists());
        assert!(!video_dir.join("Artist/Album/track.mp3").exists());

        // PUT a video file → lands in video_dir.
        let video = b"video payload".to_vec();
        let sha = hex::encode(Sha256::digest(&video));
        let req = signed(
            actix_web::http::Method::PUT,
            "/music/Movies/foo.mkv",
            "",
            &sha,
        )
        .set_payload(video.clone())
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!music_dir.join("Movies/foo.mkv").exists());
        assert!(video_dir.join("Movies/foo.mkv").exists());

        // GET on the video key finds it in video_dir.
        let req = signed(
            actix_web::http::Method::GET,
            "/music/Movies/foo.mkv",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        assert_eq!(body.as_ref(), video.as_slice());

        // LIST returns both files.
        let req = signed(
            actix_web::http::Method::GET,
            "/music",
            "list-type=2",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        let body = test::read_body(resp).await;
        let xml = std::str::from_utf8(&body).unwrap();
        assert!(xml.contains("<Key>Artist/Album/track.mp3</Key>"));
        assert!(xml.contains("<Key>Movies/foo.mkv</Key>"));

        // DELETE on the video key removes it from video_dir, leaves music_dir alone.
        let req = signed(
            actix_web::http::Method::DELETE,
            "/music/Movies/foo.mkv",
            "",
            sigv4::EMPTY_BODY_SHA256,
        )
        .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!video_dir.join("Movies/foo.mkv").exists());
        assert!(music_dir.join("Artist/Album/track.mp3").exists());
    }

    #[actix_web::test]
    async fn list_objects_on_unknown_bucket_returns_no_such_bucket() {
        let dir = tempdir("nobucket");
        let app = test::init_service(
            App::new().app_data(build_state(dir.clone())).configure(configure_routes),
        )
        .await;

        // No signature check happens for bucket name mismatch — the handler
        // short-circuits with NoSuchBucket. (Mirrors head_bucket's behavior.)
        let req = test::TestRequest::get().uri("/not-a-bucket").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = test::read_body(resp).await;
        let xml = std::str::from_utf8(&body).unwrap();
        assert!(xml.contains("<Code>NoSuchBucket</Code>"));
    }
}
