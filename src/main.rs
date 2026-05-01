// latch — single-user passkey-based auth.
//
// Recovery story: lose your device → SSH to host → `rm <creds-path>` → visit /
// from a new device → re-register. Registration mode is auto-selected when
// the creds file is empty. There is no recovery code, no fallback path.

mod config;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use tiny_http::{Header, Method, Request, Response, ResponseBox, Server};
use webauthn_rs::prelude::*;

use crate::config::Config;

const PAGE:    &str = include_str!("page.html");
const VERSION: &str = env!("CARGO_PKG_VERSION");

// Handlers return Err(message) on bad-request paths. The dispatcher in
// `handle` turns that into a 400 with the message as body.
type Resp = Result<ResponseBox, String>;

struct Session    { expires: Instant }
struct Challenge  { kind: ChKind, expires: Instant, next: String }
enum   ChKind     { Register(PasskeyRegistration), Login(PasskeyAuthentication) }

struct Latch {
    config:     Config,
    wa:         Webauthn,
    user_id:    Uuid,
    creds:      Mutex<Vec<Passkey>>,
    sessions:   Mutex<HashMap<String, Session>>,
    challenges: Mutex<HashMap<String, Challenge>>,
}

fn main() {
    match env::args().nth(1).as_deref() {
        Some("--version") | Some("-v") => { println!("latch {VERSION}"); return }
        Some("--help")    | Some("-h") => { print_help(); return }
        Some("--check") => return run_check(),
        Some(other) => {
            eprintln!("unknown argument: {other} (try --help)");
            std::process::exit(2);
        }
        None => {}
    }

    let cfg = Config::from_env();
    cfg.print();
    if let Err(e) = cfg.check() {
        eprintln!("config error: {e}");
        eprintln!("set values in /etc/latch/env (or LATCH_* env vars), then restart");
        std::process::exit(1);
    }

    let user_id = Uuid::parse_str(config::USER_ID).expect("USER_ID");
    let origin  = Url::parse(&cfg.rp_origin).expect("RP_ORIGIN");
    let wa = WebauthnBuilder::new(&cfg.rp_id, &origin)
        .expect("rp")
        .rp_name(config::RP_NAME)
        .build()
        .expect("webauthn");

    let listen = cfg.listen.clone();
    let creds  = load_creds(&cfg.creds_path);
    let latch: &'static Latch = Box::leak(Box::new(Latch {
        config: cfg,
        wa,
        user_id,
        creds:      Mutex::new(creds),
        sessions:   Mutex::new(HashMap::new()),
        challenges: Mutex::new(HashMap::new()),
    }));

    thread::spawn(move || sweeper(latch));

    let server = Server::http(&listen).expect("listen");
    for req in server.incoming_requests() {
        handle(req, latch);
    }
}

fn run_check() {
    let cfg = Config::from_env();
    cfg.print();
    if let Err(e) = cfg.check() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    eprintln!("ok");
}

fn print_help() {
    println!("latch {VERSION} — single-user passkey auth\n");
    println!("USAGE:");
    println!("  latch              run the server (configured via env)");
    println!("  latch --check      validate config and exit");
    println!("  latch --version    print version");
    println!("  latch --help       this message");
    println!();
    println!("ENVIRONMENT:");
    println!("  LATCH_RP_ID         e.g. latch.example.com");
    println!("  LATCH_RP_ORIGIN     e.g. https://latch.example.com");
    println!("  LATCH_COOKIE_DOMAIN e.g. example.com");
    println!("  LATCH_LISTEN        default 127.0.0.1:8080");
    println!("  LATCH_CREDS_PATH    default creds.json");
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

    let session = random_token();
    latch.sessions.lock().unwrap().insert(
        session.clone(),
        Session { expires: Instant::now() + config::SESSION_TTL },
    );
    let body = serde_json::json!({ "next": ch.next });
    Ok(Response::from_string(body.to_string())
        .with_header(ct("application/json"))
        .with_header(set_session_cookie(&latch.config, &session))
        .with_header(clear_challenge_cookie())
        .boxed())
}

fn logout(req: &Request, latch: &Latch) -> Resp {
    if let Some(t) = cookie(req, config::COOKIE_SESSION) {
        latch.sessions.lock().unwrap().remove(&t);
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
    save_creds(&creds, &latch.config.creds_path).map_err(|e| format!("save: {e}"))
}

fn finish_login(latch: &Latch, body: &str, state: PasskeyAuthentication) -> Result<(), String> {
    let cred: PublicKeyCredential = serde_json::from_str(body)
        .map_err(|e| format!("parse: {e}"))?;
    let result = latch.wa.finish_passkey_authentication(&cred, &state)
        .map_err(|e| format!("login: {e}"))?;
    let mut creds = latch.creds.lock().unwrap();
    if creds.iter_mut().any(|c| c.update_credential(&result).is_some()) {
        let _ = save_creds(&creds, &latch.config.creds_path);
    }
    Ok(())
}

fn session_valid(latch: &Latch, token: &str) -> bool {
    let mut sessions = latch.sessions.lock().unwrap();
    match sessions.get(token) {
        Some(s) if s.expires > Instant::now() => true,
        Some(_) => { sessions.remove(token); false }
        None    => false,
    }
}

fn sweeper(latch: &Latch) -> ! {
    loop {
        thread::sleep(config::SWEEP_INTERVAL);
        let now = Instant::now();
        latch.sessions  .lock().unwrap().retain(|_, s| s.expires > now);
        latch.challenges.lock().unwrap().retain(|_, c| c.expires > now);
    }
}

// --- persistence -----------------------------------------------------------

fn load_creds(path: &str) -> Vec<Passkey> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_creds(creds: &[Passkey], path: &str) -> std::io::Result<()> {
    let path = Path::new(path);
    let tmp  = path.with_extension("json.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(creds)?)?;
    f.sync_all()?;
    fs::rename(tmp, path)
}

// --- http ------------------------------------------------------------------

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

// --- urls ------------------------------------------------------------------

fn query_param(url: &str, key: &str) -> String {
    // url::Url rejects relative URIs; synthesize a base so we can use query_pairs().
    Url::parse(&format!("http://x{url}"))
        .ok()
        .and_then(|u| u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()))
        .unwrap_or_default()
}

// --- crypto ----------------------------------------------------------------

fn random_token() -> String {
    let mut buf = [0u8; 32];
    fs::File::open("/dev/urandom").expect("open /dev/urandom")
        .read_exact(&mut buf).expect("read /dev/urandom");
    b64u(&buf)
}

fn b64u(bytes: &[u8]) -> String {
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

// --- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            rp_id:         "latch.example.org".into(),
            rp_origin:     "https://latch.example.org".into(),
            cookie_domain: "example.org".into(),
            listen:        "127.0.0.1:0".into(),
            creds_path:    "/tmp/test-creds.json".into(),
        }
    }

    #[test]
    fn b64u_known_vectors() {
        assert_eq!(b64u(b""),       "");
        assert_eq!(b64u(b"f"),      "Zg");
        assert_eq!(b64u(b"fo"),     "Zm8");
        assert_eq!(b64u(b"foo"),    "Zm9v");
        assert_eq!(b64u(b"foob"),   "Zm9vYg");
        assert_eq!(b64u(b"fooba"),  "Zm9vYmE");
        assert_eq!(b64u(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn b64u_no_padding() {
        for n in 1..16 {
            let bytes = vec![0u8; n];
            assert!(!b64u(&bytes).contains('='), "len {n}");
        }
    }

    #[test]
    fn validate_next_relative_path() {
        let c = cfg();
        assert_eq!(c.validate_next("/foo"), "/foo");
        assert_eq!(c.validate_next("/"),    "/");
    }

    #[test]
    fn validate_next_rejects_protocol_relative() {
        assert_eq!(cfg().validate_next("//evil.com"), "/");
    }

    #[test]
    fn validate_next_rejects_external_origin() {
        assert_eq!(cfg().validate_next("https://evil.com"), "/");
    }

    #[test]
    fn validate_next_accepts_subdomain() {
        let c = cfg();
        assert_eq!(c.validate_next("https://app.example.org/dash"),
                   "https://app.example.org/dash");
    }

    #[test]
    fn validate_next_accepts_apex() {
        let c = cfg();
        assert_eq!(c.validate_next("https://example.org/"),
                   "https://example.org/");
    }

    #[test]
    fn validate_next_rejects_http_scheme() {
        assert_eq!(cfg().validate_next("http://example.org/"), "/");
    }

    #[test]
    fn query_param_basic() {
        assert_eq!(query_param("/?next=foo", "next"),                       "foo");
        assert_eq!(query_param("/?next=https%3A%2F%2Fa.b", "next"),         "https://a.b");
        assert_eq!(query_param("/", "next"),                                "");
        assert_eq!(query_param("/?other=x", "next"),                        "");
        assert_eq!(query_param("/?a=1&next=x&b=2", "next"),                 "x");
    }

    #[test]
    fn config_check_rejects_placeholder() {
        let mut c = cfg();
        c.rp_id = "latch.example.com".into();
        assert!(c.check().is_err());
    }

    #[test]
    fn config_check_accepts_real() {
        assert!(cfg().check().is_ok());
    }
}
