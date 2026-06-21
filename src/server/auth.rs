use md5::{Digest, Md5};

pub fn check(
    username: &str,
    password: &str,
    u: Option<&str>,
    p: Option<&str>,
    t: Option<&str>,
    s: Option<&str>,
) -> bool {
    let Some(req_user) = u else { return false };
    if req_user != username {
        return false;
    }

    if let (Some(token), Some(salt)) = (t, s) {
        let mut hasher = Md5::new();
        hasher.update(password.as_bytes());
        hasher.update(salt.as_bytes());
        let digest = hasher.finalize();
        let expected: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        return token == expected;
    }

    if let Some(plain) = p {
        let decoded = if let Some(hex_part) = plain.strip_prefix("enc:") {
            hex::decode(hex_part)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_else(|| plain.to_string())
        } else {
            plain.to_string()
        };
        return decoded == password;
    }

    false
}
