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
    pub access_key: Arc<String>,
    pub secret_key: Arc<String>,
}

pub async fn start(cfg: S3Config, music_dir: PathBuf) -> anyhow::Result<()> {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let state = web::Data::new(S3State {
        music_dir,
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
