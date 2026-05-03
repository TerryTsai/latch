// HTTP server. Five endpoints, all the WebAuthn ceremony work, JWT issuing.
// Spawned by `latch run` after config is loaded.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use tiny_http::{Header, Method, Request, Response, ResponseBox, Server};
use webauthn_rs::prelude::*;

use crate::config::{self, Config};
use crate::jwt;
use crate::state::{self, Revoked};

const PAGE: &str = include_str!("page.html");

type Resp = Result<ResponseBox, String>;

struct Challenge { kind: ChKind, expires: Instant, next: String }
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
    let origin  = Url::parse(&cfg.rp_origin).map_err(|e| format!("invalid rp_origin: {e}"))?;
    let wa = WebauthnBuilder::new(&cfg.rp_id, &origin)
        .map_err(|e| format!("rp: {e}"))?
        .rp_name(config::RP_NAME)
        .build()
        .map_err(|e| format!("webauthn: {e}"))?;

    let listen  = cfg.listen.clone();
    let creds   = state::load_creds(&cfg.creds_path);
    let key     = state::load_or_create_key(&cfg.key_path);
    let revoked = state::load_revoked(&cfg.revoked_path);

    let latch: &'static Latch = Box::leak(Box::new(Latch {
        config: cfg,
        wa,
        user_id,
        key,
        creds:      Mutex::new(creds),
        challenges: Mutex::new(HashMap::new()),
        revoked:    Mutex::new(revoked),
    }));

    thread::spawn(move || sweeper(latch));

    let server = Server::http(&listen).map_err(|e| format!("listen on {listen}: {e}"))?;
    for req in server.incoming_requests() {
        handle(req, latch);
    }
    Ok(())
}

fn handle(mut req: Request, latch: &Latch) {
    let path = req.url().split('?').next().unwrap_or("");
    let res: Resp = match (req.method(), path) {
        (Method::Get,  "/")         => index(),
        (Method::Get,  "/verify")   => verify(&req, latch),
        (Method::Post, "/begin")    => begin(&req, latch),
        (Method::Post, "/complete") => complete(&mut req, latch),
        (Method::Post, "/logout")   => logout(&req, latch),
        _ => Ok(Response::from_string("not found").with_status_code(404).boxed()),
    };
    let _ = req.respond(res.unwrap_or_else(|e| bad(&e)));
}

// --- handlers --------------------------------------------------------------

fn index() -> Resp {
    Ok(Response::from_string(PAGE).with_header(ct("text/html; charset=utf-8")).boxed())
}

fn verify(req: &Request, latch: &Latch) -> Resp {
    let ok = cookie(req, config::COOKIE_SESSION)
        .is_some_and(|t| session_valid(latch, &t));
    Ok(Response::empty(if ok { 200 } else { 401 }).boxed())
}

fn begin(req: &Request, latch: &Latch) -> Resp {
    let next = latch.config.validate_next(&query_param(req.url(), "next"));
    let is_first = latch.creds.lock().unwrap().is_empty();
    if is_first {
        let (ccr, state) = latch.wa.start_passkey_registration(
            latch.user_id, config::USER_NAME, config::USER_DISPLAY, None,
        ).map_err(|e| format!("register-begin: {e}"))?;
        Ok(issue_challenge(latch, "register", ChKind::Register(state), &ccr, next))
    } else {
        let creds = latch.creds.lock().unwrap();
        let (rcr, state) = latch.wa.start_passkey_authentication(&creds)
            .map_err(|e| format!("login-begin: {e}"))?;
        drop(creds);
        Ok(issue_challenge(latch, "login", ChKind::Login(state), &rcr, next))
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

    let body = serde_json::json!({ "next": ch.next });
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
    latch: &Latch, mode: &str, kind: ChKind, options: &T, next: String,
) -> ResponseBox {
    let token = random_token();
    latch.challenges.lock().unwrap().insert(
        token.clone(),
        Challenge { kind, expires: Instant::now() + config::CHALLENGE_TTL, next },
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
    state::save_creds(&creds, &latch.config.creds_path).map_err(|e| format!("save: {e}"))
}

fn finish_login(latch: &Latch, body: &str, st: PasskeyAuthentication) -> Result<(), String> {
    let cred: PublicKeyCredential = serde_json::from_str(body)
        .map_err(|e| format!("parse: {e}"))?;
    let result = latch.wa.finish_passkey_authentication(&cred, &st)
        .map_err(|e| format!("login: {e}"))?;
    let mut creds = latch.creds.lock().unwrap();
    if creds.iter_mut().any(|c| c.update_credential(&result).is_some()) {
        let _ = state::save_creds(&creds, &latch.config.creds_path);
    }
    Ok(())
}

fn session_valid(latch: &Latch, token: &str) -> bool {
    let Ok(claims) = jwt::verify(token, &latch.key) else { return false };
    !latch.revoked.lock().unwrap().contains_key(&claims.jti)
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
