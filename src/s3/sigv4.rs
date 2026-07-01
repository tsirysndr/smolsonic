use actix_web::HttpRequest;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

pub const EMPTY_BODY_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
pub const STREAMING_PAYLOAD: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD";

#[derive(Debug)]
pub struct AuthError(pub String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn verify(
    req: &HttpRequest,
    body_sha256_hex: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
) -> Result<(), AuthError> {
    let parts = parse_authorization(req)?;
    if parts.access_key != access_key {
        return Err(AuthError("access key mismatch".into()));
    }

    let canonical = build_canonical_request(req, &parts.signed_headers, body_sha256_hex)?;
    let scope = format!("{}/{}/{}/aws4_request", parts.scope_date, region, service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        parts.amz_date,
        scope,
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );

    let signing_key = derive_signing_key(secret_key, &parts.scope_date, region, service);
    let computed = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    if !constant_time_eq(computed.as_bytes(), parts.signature.as_bytes()) {
        return Err(AuthError("signature mismatch".into()));
    }
    Ok(())
}

#[derive(Debug)]
struct AuthParts {
    access_key: String,
    scope_date: String,
    amz_date: String,
    signed_headers: Vec<String>,
    signature: String,
}

fn parse_authorization(req: &HttpRequest) -> Result<AuthParts, AuthError> {
    // Try header first.
    if let Some(hdr) = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        return parse_authorization_header(hdr, req);
    }
    // Then presigned query string.
    let query: BTreeMap<String, String> = parse_query(req.query_string());
    if query.get("X-Amz-Algorithm").map(|s| s.as_str()) == Some("AWS4-HMAC-SHA256") {
        return parse_presigned(query, req);
    }
    Err(AuthError("missing authorization".into()))
}

fn parse_authorization_header(hdr: &str, req: &HttpRequest) -> Result<AuthParts, AuthError> {
    let rest = hdr
        .strip_prefix("AWS4-HMAC-SHA256")
        .ok_or_else(|| AuthError("unsupported auth algorithm".into()))?
        .trim_start();

    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = None;
    for part in rest.split(',') {
        let kv = part.trim();
        if let Some(v) = kv.strip_prefix("Credential=") {
            credential = Some(v.to_string());
        } else if let Some(v) = kv.strip_prefix("SignedHeaders=") {
            signed_headers = Some(v.to_string());
        } else if let Some(v) = kv.strip_prefix("Signature=") {
            signature = Some(v.to_string());
        }
    }

    let credential = credential.ok_or_else(|| AuthError("missing Credential".into()))?;
    let signed_headers = signed_headers.ok_or_else(|| AuthError("missing SignedHeaders".into()))?;
    let signature = signature.ok_or_else(|| AuthError("missing Signature".into()))?;

    let cred_parts: Vec<&str> = credential.split('/').collect();
    if cred_parts.len() != 5 || cred_parts[4] != "aws4_request" {
        return Err(AuthError("malformed Credential".into()));
    }

    let amz_date = req
        .headers()
        .get("x-amz-date")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AuthError("missing x-amz-date".into()))?
        .to_string();

    Ok(AuthParts {
        access_key: cred_parts[0].to_string(),
        scope_date: cred_parts[1].to_string(),
        amz_date,
        signed_headers: signed_headers
            .split(';')
            .map(|s| s.to_ascii_lowercase())
            .collect(),
        signature,
    })
}

fn parse_presigned(
    q: BTreeMap<String, String>,
    _req: &HttpRequest,
) -> Result<AuthParts, AuthError> {
    let credential = q
        .get("X-Amz-Credential")
        .ok_or_else(|| AuthError("missing X-Amz-Credential".into()))?;
    let signed_headers = q
        .get("X-Amz-SignedHeaders")
        .ok_or_else(|| AuthError("missing X-Amz-SignedHeaders".into()))?;
    let signature = q
        .get("X-Amz-Signature")
        .ok_or_else(|| AuthError("missing X-Amz-Signature".into()))?
        .clone();
    let amz_date = q
        .get("X-Amz-Date")
        .ok_or_else(|| AuthError("missing X-Amz-Date".into()))?
        .clone();
    let cred_parts: Vec<&str> = credential.split('/').collect();
    if cred_parts.len() != 5 || cred_parts[4] != "aws4_request" {
        return Err(AuthError("malformed X-Amz-Credential".into()));
    }
    Ok(AuthParts {
        access_key: cred_parts[0].to_string(),
        scope_date: cred_parts[1].to_string(),
        amz_date,
        signed_headers: signed_headers
            .split(';')
            .map(|s| s.to_ascii_lowercase())
            .collect(),
        signature,
    })
}

fn build_canonical_request(
    req: &HttpRequest,
    signed_headers: &[String],
    body_sha256_hex: &str,
) -> Result<String, AuthError> {
    let method = req.method().as_str().to_ascii_uppercase();
    let canonical_uri = canonical_uri(req.uri().path());
    let canonical_query = canonical_query_string(req.query_string());

    let mut header_lines = Vec::new();
    for name in signed_headers {
        let value = req
            .headers()
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if name == "host" {
                    req.connection_info().host().to_string()
                } else {
                    String::new()
                }
            });
        let trimmed = collapse_ws(value.trim());
        header_lines.push(format!("{}:{}\n", name, trimmed));
    }
    let canonical_headers: String = header_lines.join("");
    let signed_headers_str = signed_headers.join(";");

    Ok(format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri,
        canonical_query,
        canonical_headers,
        signed_headers_str,
        body_sha256_hex
    ))
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch == ' ' || ch == '\t' {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    // `req.uri().path()` returns the path as it was on the wire — already
    // percent-encoded. Re-encoding that directly would turn `%20` into `%2520`
    // and mismatch the client's canonical request. Decode first, then
    // single-encode per AWS S3 SigV4 rules (slashes preserved).
    let decoded = percent_decode(path);
    uri_encode(&decoded, false)
}

pub fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b;
        let unreserved =
            c.is_ascii_alphanumeric() || c == b'-' || c == b'.' || c == b'_' || c == b'~';
        if unreserved {
            out.push(c as char);
        } else if c == b'/' && !encode_slash {
            out.push('/');
        } else {
            out.push('%');
            out.push_str(&format!("{:02X}", c));
        }
    }
    out
}

fn canonical_query_string(qs: &str) -> String {
    if qs.is_empty() {
        return String::new();
    }
    // Parse the raw query into (encoded-name, encoded-value) pairs that we then sort.
    // X-Amz-Signature is excluded: for presigned URLs it is added AFTER the canonical
    // query string is built, so including it here would never match the client's signature.
    let mut entries: Vec<(String, String)> = Vec::new();
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let dk = percent_decode(k);
        if dk == "X-Amz-Signature" {
            continue;
        }
        let dv = percent_decode(v);
        entries.push((uri_encode(&dk, true), uri_encode(&dv, true)));
    }
    entries.sort();
    entries
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&")
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
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            // leave as-is (S3 does not treat '+' as space in path)
            out.push(b'+');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{}", secret);
    let k_date = hmac_sha256(k_secret.as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Decode an AWS chunked stream (STREAMING-AWS4-HMAC-SHA256-PAYLOAD) into raw bytes.
/// We do not verify per-chunk signatures — the seed signature in the Authorization
/// header was already validated, which authenticates the request as a whole.
pub fn decode_chunked_stream(body: &[u8]) -> Result<Vec<u8>, AuthError> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        // Read header line: <hex-size>;chunk-signature=<sig>\r\n
        let nl = find_crlf(body, i)
            .ok_or_else(|| AuthError("malformed chunked body: no CRLF in header".into()))?;
        let header = std::str::from_utf8(&body[i..nl])
            .map_err(|_| AuthError("malformed chunked header".into()))?;
        let size_str = header.split(';').next().unwrap_or("");
        let size = usize::from_str_radix(size_str.trim(), 16)
            .map_err(|_| AuthError(format!("bad chunk size: {size_str}")))?;
        i = nl + 2;
        if size == 0 {
            break;
        }
        if i + size > body.len() {
            return Err(AuthError("chunk extends past body".into()));
        }
        out.extend_from_slice(&body[i..i + size]);
        i += size;
        // Skip trailing CRLF after chunk data
        if i + 2 > body.len() || &body[i..i + 2] != b"\r\n" {
            return Err(AuthError("missing CRLF after chunk data".into()));
        }
        i += 2;
    }
    Ok(out)
}

fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < buf.len() {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Test-only signer that mirrors `verify`'s canonical-request construction.
/// Returns an `Authorization` header value for the given request shape.
#[cfg(test)]
#[allow(clippy::too_many_arguments, dead_code)]
pub fn sign_authorization(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(&str, &str)],
    signed_headers: &[&str],
    body_sha256_hex: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
    amz_date: &str,
    scope_date: &str,
) -> String {
    let mut header_lines = Vec::new();
    let lowered: Vec<String> = signed_headers
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    for name in &lowered {
        let value = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
            .unwrap_or("");
        let trimmed = collapse_ws(value.trim());
        header_lines.push(format!("{}:{}\n", name, trimmed));
    }
    let canonical_headers: String = header_lines.join("");
    let signed_headers_str = lowered.join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        canonical_uri(path),
        canonical_query_string(query),
        canonical_headers,
        signed_headers_str,
        body_sha256_hex
    );
    let scope = format!("{}/{}/{}/aws4_request", scope_date, region, service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date,
        scope,
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let signing_key = derive_signing_key(secret_key, scope_date, region, service);
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{}/{}/{}/aws4_request, SignedHeaders={}, Signature={}",
        access_key, scope_date, region, service, signed_headers_str, signature
    )
}
