use super::sigv4::{self, EMPTY_BODY_SHA256, STREAMING_PAYLOAD, UNSIGNED_PAYLOAD};
use super::{S3State, BUCKET, REGION, SERVICE};
use actix_web::http::header::{self, HeaderValue};
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::{DateTime, Utc};
use md5::{Digest as _, Md5};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

const MAX_KEYS_DEFAULT: usize = 1000;
const MAX_KEYS_CAP: usize = 1000;

pub async fn list_buckets(
    req: HttpRequest,
    state: web::Data<S3State>,
) -> HttpResponse {
    if let Err(e) = sigv4::verify(
        &req,
        EMPTY_BODY_SHA256,
        &state.access_key,
        &state.secret_key,
        REGION,
        SERVICE,
    ) {
        return forbidden(&e.0);
    }
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Owner><ID>smolsonic</ID><DisplayName>smolsonic</DisplayName></Owner>
  <Buckets><Bucket><Name>{BUCKET}</Name><CreationDate>1970-01-01T00:00:00.000Z</CreationDate></Bucket></Buckets>
</ListAllMyBucketsResult>"#,
    );
    xml_ok(body)
}

pub async fn list_objects(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<S3State>,
) -> HttpResponse {
    let bucket = path.into_inner();
    if bucket != BUCKET {
        return no_such_bucket(&bucket);
    }
    if let Err(e) = sigv4::verify(
        &req,
        EMPTY_BODY_SHA256,
        &state.access_key,
        &state.secret_key,
        REGION,
        SERVICE,
    ) {
        return forbidden(&e.0);
    }

    let q: BTreeMap<String, String> = parse_query(req.query_string());
    let list_type = q.get("list-type").map(|s| s.as_str()).unwrap_or("");
    let prefix = q.get("prefix").cloned().unwrap_or_default();
    let delimiter = q.get("delimiter").cloned().unwrap_or_default();
    let start_after = q.get("start-after").cloned().unwrap_or_default();
    let continuation_token = q
        .get("continuation-token")
        .cloned()
        .unwrap_or_default();
    let max_keys = q
        .get("max-keys")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(MAX_KEYS_DEFAULT)
        .min(MAX_KEYS_CAP);

    let after_marker = if !continuation_token.is_empty() {
        continuation_token.clone()
    } else {
        start_after.clone()
    };

    let mut entries: Vec<FileEntry> = match collect_entries(&state.music_dir) {
        Ok(e) => e,
        Err(e) => return internal_error(&format!("list failed: {e}")),
    };
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    let mut contents = Vec::new();
    let mut common_prefixes: Vec<String> = Vec::new();
    let mut seen_prefixes = std::collections::BTreeSet::new();
    let mut next_token: Option<String> = None;
    let mut is_truncated = false;

    for entry in entries {
        if !entry.key.starts_with(&prefix) {
            continue;
        }
        if !after_marker.is_empty() && entry.key <= after_marker {
            continue;
        }

        if !delimiter.is_empty() {
            let after = &entry.key[prefix.len()..];
            if let Some(idx) = after.find(&delimiter) {
                let cp = format!("{}{}{}", prefix, &after[..idx], delimiter);
                if seen_prefixes.insert(cp.clone()) {
                    if contents.len() + common_prefixes.len() >= max_keys {
                        is_truncated = true;
                        next_token = Some(entry.key.clone());
                        break;
                    }
                    common_prefixes.push(cp);
                }
                continue;
            }
        }

        if contents.len() + common_prefixes.len() >= max_keys {
            is_truncated = true;
            next_token = Some(entry.key.clone());
            break;
        }
        contents.push(entry);
    }

    let mut body = String::new();
    body.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    body.push_str("\n<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    body.push_str(&format!("<Name>{}</Name>", xml_escape(BUCKET)));
    body.push_str(&format!("<Prefix>{}</Prefix>", xml_escape(&prefix)));
    body.push_str(&format!("<MaxKeys>{}</MaxKeys>", max_keys));
    body.push_str(&format!("<KeyCount>{}</KeyCount>", contents.len() + common_prefixes.len()));
    body.push_str(&format!("<IsTruncated>{}</IsTruncated>", is_truncated));
    if !delimiter.is_empty() {
        body.push_str(&format!("<Delimiter>{}</Delimiter>", xml_escape(&delimiter)));
    }
    if list_type == "2" {
        if !continuation_token.is_empty() {
            body.push_str(&format!(
                "<ContinuationToken>{}</ContinuationToken>",
                xml_escape(&continuation_token)
            ));
        }
        if let Some(t) = &next_token {
            body.push_str(&format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                xml_escape(t)
            ));
        }
        if !start_after.is_empty() {
            body.push_str(&format!(
                "<StartAfter>{}</StartAfter>",
                xml_escape(&start_after)
            ));
        }
    } else if let Some(t) = &next_token {
        body.push_str(&format!("<NextMarker>{}</NextMarker>", xml_escape(t)));
    }

    for entry in &contents {
        body.push_str("<Contents>");
        body.push_str(&format!("<Key>{}</Key>", xml_escape(&entry.key)));
        body.push_str(&format!(
            "<LastModified>{}</LastModified>",
            entry.last_modified
        ));
        body.push_str(&format!("<ETag>&quot;{}&quot;</ETag>", entry.etag));
        body.push_str(&format!("<Size>{}</Size>", entry.size));
        body.push_str("<StorageClass>STANDARD</StorageClass>");
        body.push_str("</Contents>");
    }
    for cp in &common_prefixes {
        body.push_str("<CommonPrefixes>");
        body.push_str(&format!("<Prefix>{}</Prefix>", xml_escape(cp)));
        body.push_str("</CommonPrefixes>");
    }
    body.push_str("</ListBucketResult>");
    xml_ok(body)
}

pub async fn get_object(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<S3State>,
) -> HttpResponse {
    let (bucket, key) = path.into_inner();
    if bucket != BUCKET {
        return no_such_bucket(&bucket);
    }
    if let Err(e) = sigv4::verify(
        &req,
        EMPTY_BODY_SHA256,
        &state.access_key,
        &state.secret_key,
        REGION,
        SERVICE,
    ) {
        return forbidden(&e.0);
    }

    let file_path = match resolve_key(&state.music_dir, &key) {
        Some(p) => p,
        None => return bad_request("invalid key"),
    };
    match tokio::fs::read(&file_path).await {
        Ok(bytes) => {
            let etag = hex::encode(Md5::digest(&bytes));
            let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
            let modified = file_path
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| DateTime::<Utc>::from(t))
                .ok();
            let mut resp = HttpResponse::Ok();
            resp.insert_header((header::CONTENT_TYPE, mime.essence_str()));
            resp.insert_header(("ETag", format!("\"{}\"", etag)));
            if let Some(m) = modified {
                resp.insert_header((
                    header::LAST_MODIFIED,
                    HeaderValue::from_str(&m.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
                        .unwrap(),
                ));
            }
            resp.body(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => no_such_key(&key),
        Err(e) => internal_error(&format!("read: {e}")),
    }
}

pub async fn head_object(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<S3State>,
) -> HttpResponse {
    let (bucket, key) = path.into_inner();
    if bucket != BUCKET {
        return no_such_bucket(&bucket);
    }
    if let Err(e) = sigv4::verify(
        &req,
        EMPTY_BODY_SHA256,
        &state.access_key,
        &state.secret_key,
        REGION,
        SERVICE,
    ) {
        return forbidden(&e.0);
    }
    let file_path = match resolve_key(&state.music_dir, &key) {
        Some(p) => p,
        None => return bad_request("invalid key"),
    };
    match tokio::fs::metadata(&file_path).await {
        Ok(meta) if meta.is_file() => {
            let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
            let modified = meta
                .modified()
                .map(|t| DateTime::<Utc>::from(t))
                .ok();
            let mut resp = HttpResponse::Ok();
            resp.insert_header((header::CONTENT_TYPE, mime.essence_str()));
            resp.insert_header((header::CONTENT_LENGTH, meta.len()));
            if let Some(m) = modified {
                resp.insert_header((
                    header::LAST_MODIFIED,
                    HeaderValue::from_str(&m.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
                        .unwrap(),
                ));
            }
            resp.finish()
        }
        Ok(_) => no_such_key(&key),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => no_such_key(&key),
        Err(e) => internal_error(&format!("stat: {e}")),
    }
}

pub async fn put_object(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<S3State>,
    body: web::Bytes,
) -> HttpResponse {
    let (bucket, key) = path.into_inner();
    if bucket != BUCKET {
        return no_such_bucket(&bucket);
    }

    let content_sha = req
        .headers()
        .get("x-amz-content-sha256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // The Authorization signature is computed over the value of x-amz-content-sha256.
    if let Err(e) = sigv4::verify(
        &req,
        &content_sha,
        &state.access_key,
        &state.secret_key,
        REGION,
        SERVICE,
    ) {
        return forbidden(&e.0);
    }

    // Decode body according to payload signing mode.
    let payload: Vec<u8> = if content_sha == STREAMING_PAYLOAD {
        match sigv4::decode_chunked_stream(&body) {
            Ok(b) => b,
            Err(e) => return bad_request(&format!("chunked decode: {}", e.0)),
        }
    } else if content_sha == UNSIGNED_PAYLOAD || content_sha.is_empty() {
        body.to_vec()
    } else {
        let computed = hex::encode(Sha256::digest(&body));
        if !computed.eq_ignore_ascii_case(&content_sha) {
            return bad_request("payload sha256 mismatch");
        }
        body.to_vec()
    };

    let file_path = match resolve_key(&state.music_dir, &key) {
        Some(p) => p,
        None => return bad_request("invalid key"),
    };
    if let Some(parent) = file_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return internal_error(&format!("mkdir: {e}"));
        }
    }
    if let Err(e) = tokio::fs::write(&file_path, &payload).await {
        return internal_error(&format!("write: {e}"));
    }

    let etag = hex::encode(Md5::digest(&payload));
    let mut resp = HttpResponse::Ok();
    resp.insert_header(("ETag", format!("\"{}\"", etag)));
    resp.finish()
}

pub async fn delete_object(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<S3State>,
) -> HttpResponse {
    let (bucket, key) = path.into_inner();
    if bucket != BUCKET {
        return no_such_bucket(&bucket);
    }
    if let Err(e) = sigv4::verify(
        &req,
        EMPTY_BODY_SHA256,
        &state.access_key,
        &state.secret_key,
        REGION,
        SERVICE,
    ) {
        return forbidden(&e.0);
    }
    let file_path = match resolve_key(&state.music_dir, &key) {
        Some(p) => p,
        None => return bad_request("invalid key"),
    };
    match tokio::fs::remove_file(&file_path).await {
        Ok(_) => {
            cleanup_empty_dirs(&state.music_dir, file_path.parent()).await;
            HttpResponse::NoContent().finish()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HttpResponse::NoContent().finish(),
        Err(e) => internal_error(&format!("delete: {e}")),
    }
}

async fn cleanup_empty_dirs(root: &Path, mut dir: Option<&Path>) {
    while let Some(d) = dir {
        if d == root {
            return;
        }
        match tokio::fs::read_dir(d).await {
            Ok(mut entries) => match entries.next_entry().await {
                Ok(Some(_)) => return,
                Ok(None) => {
                    let _ = tokio::fs::remove_dir(d).await;
                }
                Err(_) => return,
            },
            Err(_) => return,
        }
        dir = d.parent();
    }
}

struct FileEntry {
    key: String,
    size: u64,
    etag: String,
    last_modified: String,
}

fn collect_entries(music_dir: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(music_dir).follow_links(true) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(music_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut key = String::new();
        for (i, comp) in rel.components().enumerate() {
            if i > 0 {
                key.push('/');
            }
            key.push_str(&comp.as_os_str().to_string_lossy());
        }
        let meta = entry.metadata().map_err(std::io::Error::other)?;
        let size = meta.len();
        let last_modified = meta
            .modified()
            .ok()
            .map(|t| DateTime::<Utc>::from(t))
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
            .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string());
        let etag = format!("{:x}{:x}", size, meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0));
        out.push(FileEntry {
            key,
            size,
            etag,
            last_modified,
        });
    }
    Ok(out)
}

fn resolve_key(music_dir: &Path, key: &str) -> Option<PathBuf> {
    if key.is_empty() || key.starts_with('/') {
        return None;
    }
    let mut p = PathBuf::from(music_dir);
    for segment in key.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        let pb = PathBuf::from(segment);
        for comp in pb.components() {
            match comp {
                Component::Normal(s) => p.push(s),
                _ => return None,
            }
        }
    }
    Some(p)
}

fn parse_query(qs: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        out.insert(
            urlencoding::decode(k).map(|s| s.into_owned()).unwrap_or_default(),
            urlencoding::decode(v).map(|s| s.into_owned()).unwrap_or_default(),
        );
    }
    out
}

fn xml_escape(s: &str) -> String {
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

fn xml_ok(body: String) -> HttpResponse {
    HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, "application/xml"))
        .body(body)
}

fn error_xml(code: &str, message: &str, resource: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>{}</Code><Message>{}</Message><Resource>{}</Resource><RequestId>smolsonic</RequestId></Error>"#,
        xml_escape(code),
        xml_escape(message),
        xml_escape(resource)
    )
}

fn forbidden(msg: &str) -> HttpResponse {
    HttpResponse::Forbidden()
        .insert_header((header::CONTENT_TYPE, "application/xml"))
        .body(error_xml("SignatureDoesNotMatch", msg, ""))
}

fn bad_request(msg: &str) -> HttpResponse {
    HttpResponse::BadRequest()
        .insert_header((header::CONTENT_TYPE, "application/xml"))
        .body(error_xml("InvalidRequest", msg, ""))
}

fn internal_error(msg: &str) -> HttpResponse {
    HttpResponse::InternalServerError()
        .insert_header((header::CONTENT_TYPE, "application/xml"))
        .body(error_xml("InternalError", msg, ""))
}

fn no_such_bucket(bucket: &str) -> HttpResponse {
    HttpResponse::NotFound()
        .insert_header((header::CONTENT_TYPE, "application/xml"))
        .body(error_xml("NoSuchBucket", "bucket does not exist", bucket))
}

fn no_such_key(key: &str) -> HttpResponse {
    HttpResponse::NotFound()
        .insert_header((header::CONTENT_TYPE, "application/xml"))
        .body(error_xml("NoSuchKey", "key does not exist", key))
}
