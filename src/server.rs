// HTTP server. Six endpoints, all the WebAuthn ceremony work, JWT issuing.
// Spawned by `latch run` after config is loaded.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use tiny_http::{Header, Method, Request, Response, ResponseBox, Server};
use webauthn_rs::prelude::*;

use crate::config::{self, Config};
use crate::jwt;
use crate::state::{self, Revoked};

const PAGE: &str = include_str!("page.html");

type Resp = Result<ResponseBox, String>;

struct Challenge { kind: ChKind, expires: Instant, return_to: String }
enum   ChKind    { Register(PasskeyRegistration), Login(PasskeyAuthentication) }

pub struct Latch {
    config:     Config,
    wa:         Webauthn,
    user_id:    Uuid,
    key:        Vec<u8>,
    creds:      Mutex<Vec<Passkey>>,
    challenges: Mutex<HashMap<String, Challenge>>,
    revoked:    Mutex<Revoked>,
}

pub fn run(cfg: Config) -> Result<(), String> {
    let user_id = Uuid::parse_str(config::USER_ID).expect("USER_ID");
    let origin  = Url::parse(&cfg.origin).map_err(|e| format!("invalid origin: {e}"))?;
    // webauthn-rs calls this "rp_id" at its API boundary; we pass our hostname.
    let wa = WebauthnBuilder::new(&cfg.hostname, &origin)
        .map_err(|e| format!("rp: {e}"))?
        .rp_name(config::RP_NAME)
        .build()
        .map_err(|e| format!("webauthn: {e}"))?;

    let listen = cfg.listen.clone();
    std::fs::create_dir_all(&cfg.data_dir)
        .map_err(|e| format!("mkdir {}: {e}", cfg.data_dir.display()))?;
    let passkeys = state::load_passkeys(&cfg.passkeys_path);
    let key      = state::load_or_create_key(&cfg.key_path);
    let revoked  = state::load_revoked(&cfg.revoked_path);

    let latch: &'static Latch = Box::leak(Box::new(Latch {
        config: cfg,
        wa,
        user_id,
        key,
        creds:      Mutex::new(passkeys),
        challenges: Mutex::new(HashMap::new()),
        revoked:    Mutex::new(revoked),
    }));

    thread::spawn(move || sweeper(latch));

    let server = Server::http(&listen).map_err(|e| format!("listen on {listen}: {e}"))?;
    install_signal_handlers();

    // Poll with a short timeout so we can react to SIGTERM/SIGINT instead
    // of blocking forever in incoming_requests().
    while !SHUTDOWN.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(req)) => handle(req, latch),
            Ok(None)      => {}                           // timeout, loop and re-check
            Err(e) => {
                eprintln!("recv: {e}");
                break;
            }
        }
    }
    eprintln!("shutting down");
    Ok(())
}

// --- signal handling -------------------------------------------------------
//
// Installs handlers for SIGTERM and SIGINT that flip an atomic flag the
// serve loop polls. No third-party crates: raw libc::sigaction with a
// trivial async-signal-safe handler.

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: i32) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

fn install_signal_handlers() {
    const SIGINT:  i32 = 2;
    const SIGTERM: i32 = 15;
    // SAFETY: signal() is async-signal-safe; on_signal does only an atomic store.
    let h = on_signal as *const () as usize;
    unsafe {
        libc_signal(SIGINT,  h);
        libc_signal(SIGTERM, h);
    }
}

extern "C" {
    #[link_name = "signal"]
    fn libc_signal(signum: i32, handler: usize) -> usize;
}

fn handle(mut req: Request, latch: &Latch) {
    let path = req.url().split('?').next().unwrap_or("");
    let res: Resp = match (req.method(), path) {
        (Method::Get,  "/") | (Method::Get, "/login") => index(latch),
        (Method::Get,  "/verify")   => verify(&req, latch),
        (Method::Post, "/begin")    => begin(&req, latch),
        (Method::Post, "/complete") => complete(&mut req, latch),
        (Method::Post, "/logout")   => logout(&req, latch),
        _ => Ok(Response::from_string("not found").with_status_code(404).boxed()),
    };
    let _ = req.respond(res.unwrap_or_else(|e| bad(&e)));
}

// --- handlers --------------------------------------------------------------

fn index(latch: &Latch) -> Resp {
    let label = if latch.creds.lock().unwrap().is_empty() { "register passkey" } else { "sign in" };
    let html = PAGE.replace("{{label}}", label);
    Ok(Response::from_string(html).with_header(ct("text/html; charset=utf-8")).boxed())
}

// Three-way response per the forward_auth contract:
//   authed              → 200 + X-Forwarded-User
//   unauthed + browser  → 302 to /login?return_to=<original URL>
//   unauthed + API      → 401 + JSON body
fn verify(req: &Request, latch: &Latch) -> Resp {
    let claims = cookie(req, config::COOKIE_SESSION)
        .as_deref()
        .and_then(|t| jwt::verify(t, &latch.key).ok())
        .filter(|c| !latch.revoked.lock().unwrap().contains_key(&c.jti));

    if let Some(c) = claims {
        return Ok(Response::empty(200)
            .with_header(hdr("X-Forwarded-User", &c.sub))
            .boxed());
    }

    if is_api_request(req) {
        return Ok(Response::from_string(r#"{"error":"unauthenticated"}"#)
            .with_status_code(401)
            .with_header(ct("application/json"))
            .boxed());
    }

    let return_to = build_return_to(req, &latch.config);
    let location = format!(
        "https://{}/login?return_to={}",
        latch.config.hostname,
        url_encode(&return_to),
    );
    Ok(Response::empty(302).with_header(hdr("Location", &location)).boxed())
}

fn begin(req: &Request, latch: &Latch) -> Resp {
    let return_to = latch.config.validate_next(&query_param(req.url(), "return_to"));
    let is_first = latch.creds.lock().unwrap().is_empty();
    if is_first {
        let (ccr, state) = latch.wa.start_passkey_registration(
            latch.user_id, config::USER_NAME, config::USER_DISPLAY, None,
        ).map_err(|e| format!("register-begin: {e}"))?;
        Ok(issue_challenge(latch, "register", ChKind::Register(state), &ccr, return_to))
    } else {
        let creds = latch.creds.lock().unwrap();
        let (rcr, state) = latch.wa.start_passkey_authentication(&creds)
            .map_err(|e| format!("login-begin: {e}"))?;
        drop(creds);
        Ok(issue_challenge(latch, "login", ChKind::Login(state), &rcr, return_to))
    }
}

fn complete(req: &mut Request, latch: &Latch) -> Resp {
    let token = cookie(req, config::COOKIE_CHALLENGE).ok_or("no challenge cookie")?;
    let ch = latch.challenges.lock().unwrap().remove(&token)
        .filter(|c| c.expires > Instant::now())
        .ok_or("expired challenge")?;

    let mut body = String::new();
    req.as_reader().read_to_string(&mut body).map_err(|e| format!("read: {e}"))?;

    match ch.kind {
        ChKind::Register(state) => finish_register(latch, &body, state)?,
        ChKind::Login(state)    => finish_login(latch, &body, state)?,
    }

    let now = jwt::unix_now();
    let claims = jwt::Claims {
        sub: config::USER_NAME.into(),
        iat: now,
        exp: now + config::SESSION_TTL.as_secs(),
        jti: random_token(),
    };
    let token = jwt::issue(&claims, &latch.key).map_err(|e| format!("issue: {e}"))?;

    let body = serde_json::json!({ "return_to": ch.return_to });
    Ok(Response::from_string(body.to_string())
        .with_header(ct("application/json"))
        .with_header(set_session_cookie(&latch.config, &token))
        .with_header(clear_challenge_cookie())
        .boxed())
}

fn logout(req: &Request, latch: &Latch) -> Resp {
    if let Some(token) = cookie(req, config::COOKIE_SESSION) {
        if let Ok(claims) = jwt::verify(&token, &latch.key) {
            latch.revoked.lock().unwrap().insert(claims.jti, claims.exp);
            let _ = state::save_revoked(
                &latch.revoked.lock().unwrap(),
                &latch.config.revoked_path,
            );
        }
    }
    Ok(Response::empty(204).with_header(clear_session_cookie(&latch.config)).boxed())
}

// --- ceremony --------------------------------------------------------------

fn issue_challenge<T: serde::Serialize>(
    latch: &Latch, mode: &str, kind: ChKind, options: &T, return_to: String,
) -> ResponseBox {
    let token = random_token();
    latch.challenges.lock().unwrap().insert(
        token.clone(),
        Challenge { kind, expires: Instant::now() + config::CHALLENGE_TTL, return_to },
    );
    let body = serde_json::json!({ "mode": mode, "options": options });
    Response::from_string(body.to_string())
        .with_header(ct("application/json"))
        .with_header(set_challenge_cookie(&token))
        .boxed()
}

fn finish_register(latch: &Latch, body: &str, state: PasskeyRegistration) -> Result<(), String> {
    let cred: RegisterPublicKeyCredential = serde_json::from_str(body)
        .map_err(|e| format!("parse: {e}"))?;
    let pk = latch.wa.finish_passkey_registration(&cred, &state)
        .map_err(|e| format!("register: {e}"))?;
    let mut creds = latch.creds.lock().unwrap();
    creds.push(pk);
    state::save_passkeys(&creds, &latch.config.passkeys_path).map_err(|e| format!("save: {e}"))
}

fn finish_login(latch: &Latch, body: &str, st: PasskeyAuthentication) -> Result<(), String> {
    let cred: PublicKeyCredential = serde_json::from_str(body)
        .map_err(|e| format!("parse: {e}"))?;
    let result = latch.wa.finish_passkey_authentication(&cred, &st)
        .map_err(|e| format!("login: {e}"))?;
    let mut creds = latch.creds.lock().unwrap();
    if creds.iter_mut().any(|c| c.update_credential(&result).is_some()) {
        let _ = state::save_passkeys(&creds, &latch.config.passkeys_path);
    }
    Ok(())
}

fn sweeper(latch: &Latch) -> ! {
    loop {
        thread::sleep(config::SWEEP_INTERVAL);
        let now_instant = Instant::now();
        let now_unix    = jwt::unix_now();

        latch.challenges.lock().unwrap().retain(|_, c| c.expires > now_instant);

        let mut rev = latch.revoked.lock().unwrap();
        let before = rev.len();
        rev.retain(|_, &mut exp| exp > now_unix);
        if rev.len() != before {
            let _ = state::save_revoked(&rev, &latch.config.revoked_path);
        }
    }
}

// --- http helpers ----------------------------------------------------------

fn cookie(req: &Request, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    req.headers().iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("cookie"))?
        .value.as_str()
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix(&prefix).map(String::from))
}

fn header_value(req: &Request, name: &str) -> Option<String> {
    req.headers().iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn is_api_request(req: &Request) -> bool {
    if header_value(req, "X-Requested-With")
        .is_some_and(|v| v.eq_ignore_ascii_case("XMLHttpRequest"))
    { return true; }
    let accept = header_value(req, "Accept").unwrap_or_default();
    accept.contains("application/json") && !accept.contains("text/html")
}

// Reconstruct the URL the user was trying to reach from the X-Forwarded-*
// headers Caddy sends on a forward_auth subrequest. Hardcode https because
// cloudflared terminates TLS at the edge and forwards http inside the tunnel.
fn build_return_to(req: &Request, cfg: &Config) -> String {
    let host = header_value(req, "X-Forwarded-Host").unwrap_or_else(|| cfg.hostname.clone());
    let uri  = header_value(req, "X-Forwarded-Uri").unwrap_or_else(|| "/".into());
    format!("https://{host}{uri}")
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn set_session_cookie(cfg: &Config, value: &str) -> Header {
    hdr("Set-Cookie", &format!(
        "{}={}; Domain={}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}",
        config::COOKIE_SESSION, value, cfg.cookie_domain, config::SESSION_TTL.as_secs(),
    ))
}

fn clear_session_cookie(cfg: &Config) -> Header {
    hdr("Set-Cookie", &format!(
        "{}=; Domain={}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
        config::COOKIE_SESSION, cfg.cookie_domain,
    ))
}

fn set_challenge_cookie(value: &str) -> Header {
    hdr("Set-Cookie", &format!(
        "{}={}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
        config::COOKIE_CHALLENGE, value, config::CHALLENGE_TTL.as_secs(),
    ))
}

fn clear_challenge_cookie() -> Header {
    hdr("Set-Cookie", &format!(
        "{}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0",
        config::COOKIE_CHALLENGE,
    ))
}

fn hdr(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn ct(v: &str) -> Header { hdr("Content-Type", v) }

fn bad(msg: &str) -> ResponseBox {
    Response::from_string(msg).with_status_code(400).boxed()
}

fn query_param(url: &str, key: &str) -> String {
    Url::parse(&format!("http://x{url}"))
        .ok()
        .and_then(|u| u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()))
        .unwrap_or_default()
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom").expect("open /dev/urandom")
        .read_exact(&mut buf).expect("read /dev/urandom");
    jwt::b64u_encode(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("https://temp.terrytsai.dev/x?y=1"),
                   "https%3A%2F%2Ftemp.terrytsai.dev%2Fx%3Fy%3D1");
    }
}
