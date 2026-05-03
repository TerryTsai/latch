// Minimal JWT (HS256) implementation. Uses OpenSSL (already vendored) for
// HMAC-SHA256 and constant-time comparison; everything else is hand-rolled
// to avoid pulling in a JWT crate's transitive deps.
//
// Format: <header>.<payload>.<signature>
// header is fixed: {"alg":"HS256","typ":"JWT"} → constant base64url string.

use std::time::{SystemTime, UNIX_EPOCH};

use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use serde::{Deserialize, Serialize};

const HEADER: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iat: u64,
    pub exp: u64,
    pub jti: String,
}

pub fn issue(claims: &Claims, key: &[u8]) -> Result<String, String> {
    let payload_json = serde_json::to_vec(claims).map_err(|e| e.to_string())?;
    let payload = b64u_encode(&payload_json);
    let signing_input = format!("{HEADER}.{payload}");
    let sig = hmac_sha256(key, signing_input.as_bytes()).map_err(|e| e.to_string())?;
    Ok(format!("{signing_input}.{}", b64u_encode(&sig)))
}

pub fn verify(token: &str, key: &[u8]) -> Result<Claims, String> {
    let mut parts = token.splitn(4, '.');
    let header  = parts.next().ok_or("missing header")?;
    let payload = parts.next().ok_or("missing payload")?;
    let sig     = parts.next().ok_or("missing signature")?;
    if parts.next().is_some() { return Err("malformed".into()); }
    if header != HEADER { return Err("bad header".into()); }

    let signing_input = format!("{header}.{payload}");
    let expected = hmac_sha256(key, signing_input.as_bytes()).map_err(|e| e.to_string())?;
    let actual = b64u_decode(sig).ok_or("bad signature encoding")?;
    if !openssl::memcmp::eq(&expected, &actual) {
        return Err("invalid signature".into());
    }

    let payload_bytes = b64u_decode(payload).ok_or("bad payload encoding")?;
    let claims: Claims = serde_json::from_slice(&payload_bytes).map_err(|e| e.to_string())?;

    if claims.exp <= unix_now() { return Err("expired".into()); }

    Ok(claims)
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .expect("system time before unix epoch")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, openssl::error::ErrorStack> {
    let pkey = PKey::hmac(key)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &pkey)?;
    signer.update(data)?;
    signer.sign_to_vec()
}

pub fn b64u_encode(bytes: &[u8]) -> String {
    const C: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4 + 2) / 3);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = (bytes[i] as u32) << 16 | (bytes[i+1] as u32) << 8 | bytes[i+2] as u32;
        out.push(C[((n >> 18) & 63) as usize] as char);
        out.push(C[((n >> 12) & 63) as usize] as char);
        out.push(C[((n >>  6) & 63) as usize] as char);
        out.push(C[( n        & 63) as usize] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let n = (bytes[i] as u32) << 16;
            out.push(C[((n >> 18) & 63) as usize] as char);
            out.push(C[((n >> 12) & 63) as usize] as char);
        }
        2 => {
            let n = (bytes[i] as u32) << 16 | (bytes[i+1] as u32) << 8;
            out.push(C[((n >> 18) & 63) as usize] as char);
            out.push(C[((n >> 12) & 63) as usize] as char);
            out.push(C[((n >>  6) & 63) as usize] as char);
        }
        _ => {}
    }
    out
}

pub fn b64u_decode(s: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity((bytes.len() * 3 + 3) / 4);
    let mut i = 0;
    while i < bytes.len() {
        let n = (bytes.len() - i).min(4);
        if n < 2 { return None; }
        let mut combined: u32 = 0;
        for j in 0..n {
            combined = (combined << 6) | val(bytes[i + j])? as u32;
        }
        combined <<= (4 - n) * 6;
        out.push(((combined >> 16) & 0xff) as u8);
        if n > 2 { out.push(((combined >> 8) & 0xff) as u8); }
        if n > 3 { out.push((combined & 0xff) as u8); }
        i += 4;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> { (0u8..32).collect() }

    fn sample_claims(exp_offset: i64) -> Claims {
        let now = unix_now();
        Claims {
            sub: "me".into(),
            iat: now,
            exp: (now as i64 + exp_offset) as u64,
            jti: "test-jti".into(),
        }
    }

    #[test]
    fn b64u_roundtrip() {
        for n in 0..16 {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            let enc = b64u_encode(&bytes);
            assert!(!enc.contains('='), "no padding for len {n}");
            assert_eq!(b64u_decode(&enc), Some(bytes), "roundtrip len {n}");
        }
    }

    #[test]
    fn b64u_known_vectors() {
        assert_eq!(b64u_encode(b""), "");
        assert_eq!(b64u_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64u_decode("Zm9vYmFy"), Some(b"foobar".to_vec()));
    }

    #[test]
    fn issue_verify_roundtrip() {
        let claims = sample_claims(60);
        let token = issue(&claims, &key()).unwrap();
        let decoded = verify(&token, &key()).unwrap();
        assert_eq!(decoded.sub, "me");
        assert_eq!(decoded.jti, "test-jti");
        assert_eq!(decoded.exp, claims.exp);
    }

    #[test]
    fn rejects_expired() {
        let claims = sample_claims(-1);
        let token = issue(&claims, &key()).unwrap();
        assert!(verify(&token, &key()).unwrap_err().contains("expired"));
    }

    #[test]
    fn rejects_bad_signature() {
        let token = issue(&sample_claims(60), &key()).unwrap();
        let bad_key: Vec<u8> = (1u8..33).collect();
        assert!(verify(&token, &bad_key).unwrap_err().contains("invalid signature"));
    }

    #[test]
    fn rejects_malformed() {
        assert!(verify("not-a-jwt", &key()).is_err());
        assert!(verify("a.b", &key()).is_err());
        assert!(verify("a.b.c.d", &key()).is_err());
    }

    #[test]
    fn rejects_tampered_payload() {
        let token = issue(&sample_claims(60), &key()).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        let other = Claims {
            sub: "attacker".into(),
            iat: unix_now(),
            exp: unix_now() + 60,
            jti: "different".into(),
        };
        let other_payload = b64u_encode(&serde_json::to_vec(&other).unwrap());
        let tampered = format!("{}.{}.{}", parts[0], other_payload, parts[2]);
        assert!(verify(&tampered, &key()).unwrap_err().contains("invalid signature"));
    }
}
