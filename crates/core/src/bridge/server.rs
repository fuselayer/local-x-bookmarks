//! The loopback receiver.
//!
//! ## Threat model, stated plainly
//!
//! Chromium's Local Network Access restriction exists because pages attacking
//! localhost services is a real threat class. Page-context `fetch` to loopback
//! is already blocked by x.com's own CSP and by LNA, and the userscript reaches
//! us through `GM_xmlhttpRequest` in the *extension* context instead. So this
//! endpoint is built as though a hostile local caller will eventually reach it:
//!
//! - **Loopback bind only.** `127.0.0.1`, never `0.0.0.0`.
//! - **Nothing listens unless the app is open.** The socket is a child of the
//!   app's lifetime, not a service that survives it.
//! - **The bearer secret is the only authentication.** No origin check, no
//!   cookie, nothing ambient. An unauthenticated request gets a rejection and
//!   nothing else.
//! - **No CORS headers, deliberately.** Requests arrive from an extension
//!   context where CORS does not apply. Pinning an origin would be defending
//!   against a mechanism that is not in play while implying that it is.
//! - **Bounded work.** Body cap, head cap, read and write timeouts, and serial
//!   handling, so a hostile client cannot pin arbitrary memory or thread count.
//!
//! ## The one unauthenticated endpoint
//!
//! `/v1/hello` exists so the script can find the app after the port moves —
//! see [`BridgeOptions`]. It is a **pure echo**: the caller sends a random
//! `probe` value and the app sends it back with its name. The app therefore
//! discloses nothing the caller did not already supply, which is what makes it
//! safe to leave unauthenticated. In particular it reveals no version, no
//! library path, no counts and no secret.
//!
//! Without it, the script would have to send its install secret to every port
//! in the fallback range to find the app — handing the secret to whichever
//! unrelated process happens to be squatting on the wrong port. Echoing is
//! strictly better than that.

use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::import::ImportSummary;

use super::http::{self, ReadError, Request};
use super::pairing::Pairing;

/// Default port. High, unregistered, and memorable enough to recognise.
pub const DEFAULT_PORT: u16 = 8737;

/// How many ports to try after the preferred one before giving up.
pub const PORT_FALLBACK: u16 = 20;

/// How long an idle connection may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a response write may block.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the accept loop sleeps between polls while shutting down.
const POLL_INTERVAL: Duration = Duration::from_millis(40);

/// Cap on problem strings echoed back, so one bad import cannot produce a
/// megabyte response.
const MAX_REPORTED_PROBLEMS: usize = 20;

/// What the app does with a batch of captured NDJSON.
///
/// A closure rather than a database handle, so this module stays ignorant of
/// storage and can be tested against an in-memory library.
pub type Ingest = Arc<dyn Fn(&str, i64) -> Result<ImportSummary> + Send + Sync>;

/// Called on the server thread when something worth showing the user happens.
pub type Notify = Arc<dyn Fn(BridgeEvent) + Send + Sync>;

#[derive(Debug, Clone, Default, Serialize)]
pub struct CaptureReport {
    pub seen: usize,
    pub new: usize,
    pub updated: usize,
    pub problems: Vec<String>,
}

impl CaptureReport {
    fn from_summary(s: ImportSummary) -> Self {
        let total = s.problems.len();
        let mut problems: Vec<String> = s.problems.into_iter().take(MAX_REPORTED_PROBLEMS).collect();
        if total > MAX_REPORTED_PROBLEMS {
            problems.push(format!("...and {} more", total - MAX_REPORTED_PROBLEMS));
        }
        Self {
            seen: s.stats.seen,
            new: s.stats.new,
            updated: s.stats.updated,
            problems,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// A script exchanged a pairing code successfully.
    Paired { label: Option<String>, at: i64 },
    /// A batch was ingested.
    Captured(CaptureReport),
    /// A request was refused. Surfaced because a wrong secret is something the
    /// user needs to be told about, not silently dropped.
    Rejected { reason: String, at: i64 },
}

#[derive(Debug, Clone)]
pub struct BridgeOptions {
    pub preferred_port: u16,
    pub fallback: u16,
}

impl Default for BridgeOptions {
    fn default() -> Self {
        Self { preferred_port: DEFAULT_PORT, fallback: PORT_FALLBACK }
    }
}

/// A running receiver. Dropping it without [`Bridge::stop`] leaves the thread
/// alive until the process exits, which is what the app wants; tests call
/// `stop()`.
pub struct Bridge {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Bridge {
    /// Bind and start serving.
    ///
    /// Tries `preferred_port`, then the next `fallback` ports. The chosen port
    /// is reported in every response so a script that guessed wrong can correct
    /// itself on the next exchange rather than the user re-pairing.
    pub fn start(
        pairing: Arc<Mutex<Pairing>>,
        ingest: Ingest,
        notify: Notify,
        opts: BridgeOptions,
    ) -> Result<Self> {
        let (listener, port) = bind_with_fallback(opts.preferred_port, opts.fallback)?;
        listener.set_nonblocking(true).map_err(|e| Error::io("<listener>", e))?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let thread = std::thread::Builder::new()
            .name("xdl-bridge".into())
            .spawn(move || {
                let ctx = Ctx { port, pairing, ingest, notify };
                // Errors here are per-connection and already answered to the
                // client. The loop itself only exits on shutdown.
                while !thread_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(e) = serve_one(stream, &ctx) {
                                // A client that vanishes mid-request is normal.
                                let _ = e;
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(POLL_INTERVAL);
                        }
                        Err(_) => std::thread::sleep(POLL_INTERVAL),
                    }
                }
            })
            .map_err(|e| Error::io("<bridge thread>", e))?;

        Ok(Self { port, stop, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop serving and wait for the thread to finish.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Bind `preferred`, or the next free port within `fallback`.
fn bind_with_fallback(preferred: u16, fallback: u16) -> Result<(TcpListener, u16)> {
    let mut last: Option<std::io::Error> = None;
    for offset in 0..=fallback {
        let Some(port) = preferred.checked_add(offset) else { break };
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        match TcpListener::bind(addr) {
            // Report the port the OS actually bound, not the one we asked for:
            // tests pass 0 to get an arbitrary free port, and the app
            // advertises this value to the script.
            Ok(l) => {
                let bound = l.local_addr().map(|a| a.port()).unwrap_or(port);
                return Ok((l, bound));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(Error::invalid(format!(
        "could not bind any port in {}..={} on loopback: {}",
        preferred,
        preferred.saturating_add(fallback),
        last.map(|e| e.to_string()).unwrap_or_else(|| "unknown".into())
    )))
}

struct Ctx {
    port: u16,
    pairing: Arc<Mutex<Pairing>>,
    ingest: Ingest,
    notify: Notify,
}

fn serve_one(stream: TcpStream, ctx: &Ctx) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let req = match http::read_request_with(&mut reader, &mut |headers| {
        // Answer `Expect: 100-continue` before blocking on the body. .NET sends
        // this by default for POSTs and will otherwise sit out its own timeout
        // waiting for permission we never give.
        let wants_continue = headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("expect") && v.eq_ignore_ascii_case("100-continue")
        });
        if wants_continue {
            writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            writer.flush()?;
        }
        Ok(())
    }) {
        Ok(Some(req)) => req,
        // Connected and said nothing. Port probes do this constantly.
        Ok(None) => return Ok(()),
        Err(ReadError::TooLarge(m)) => {
            return http::write_json(&mut writer, 413, &err_json(&m));
        }
        Err(ReadError::UnsupportedFraming(m)) | Err(ReadError::Malformed(m)) => {
            return http::write_json(&mut writer, 400, &err_json(&m));
        }
        Err(ReadError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock
            || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            return http::write_json(&mut writer, 408, &err_json("timed out reading request"));
        }
        Err(_) => return Ok(()),
    };

    match route(&req, ctx) {
        Ok((status, body)) => http::write_json(&mut writer, status, &body),
        Err((status, message)) => {
            if status == 401 {
                (ctx.notify)(BridgeEvent::Rejected {
                    reason: message.clone(),
                    at: crate::now(),
                });
            }
            http::write_json(&mut writer, status, &err_json(&message))
        }
    }
}

fn err_json(message: &str) -> serde_json::Value {
    serde_json::json!({ "ok": false, "error": message })
}

/// Route one request. `Err((status, message))` is a refusal with a reason.
fn route(req: &Request, ctx: &Ctx) -> std::result::Result<(u16, serde_json::Value), (u16, String)> {
    match (req.method.as_str(), req.path()) {
        // ---- unauthenticated, and deliberately says nothing ----
        ("GET", "/v1/hello") => Ok((
            200,
            serde_json::json!({
                "app": "xitter-dl",
                "v": 1,
                "probe": probe_value(req.query()),
            }),
        )),

        // ---- exchange a code for the persistent secret ----
        ("POST", "/v1/pair") => {
            let body: serde_json::Value = serde_json::from_str(req.text().unwrap_or(""))
                .map_err(|_| (400u16, "expected a JSON body".to_string()))?;

            let code = body
                .get("code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| (400u16, "missing \"code\"".to_string()))?;

            let label = body
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.chars().take(120).collect::<String>());

            let now = crate::now();
            let secret = {
                let mut p = ctx.pairing.lock().map_err(|_| (500u16, "state poisoned".into()))?;
                p.redeem(code, label.clone(), now).map_err(|e| (401u16, e.to_string()))?
            };

            (ctx.notify)(BridgeEvent::Paired { label, at: now });

            Ok((
                200,
                serde_json::json!({ "ok": true, "v": 1, "secret": secret, "port": ctx.port }),
            ))
        }

        // ---- authenticated: is this secret still good? ----
        ("GET", "/v1/status") => {
            authenticate(req, ctx)?;
            let paired = ctx
                .pairing
                .lock()
                .map(|p| p.is_paired())
                .unwrap_or(false);
            Ok((200, serde_json::json!({ "ok": true, "v": 1, "paired": paired, "port": ctx.port })))
        }

        // ---- authenticated: the actual capture handoff ----
        ("POST", "/v1/capture") => {
            authenticate(req, ctx)?;

            let body = req
                .text()
                .ok_or_else(|| (400u16, "body is not valid UTF-8".to_string()))?;
            if body.trim().is_empty() {
                return Err((400, "empty body".into()));
            }

            let now = crate::now();
            let summary = (ctx.ingest)(body, now).map_err(|e| (400u16, e.to_string()))?;
            let report = CaptureReport::from_summary(summary);
            (ctx.notify)(BridgeEvent::Captured(report.clone()));

            let mut value = serde_json::to_value(&report).unwrap_or_else(|_| serde_json::json!({}));
            value["ok"] = serde_json::json!(true);
            value["port"] = serde_json::json!(ctx.port);
            Ok((200, value))
        }

        // ---- known path, wrong method ----
        (_, "/v1/hello") | (_, "/v1/pair") | (_, "/v1/status") | (_, "/v1/capture") => {
            Err((405, format!("{} is not allowed here", req.method)))
        }

        _ => Err((404, "no such endpoint".into())),
    }
}

/// The bearer secret is the only authentication. Anything else is refused
/// before the body is looked at.
fn authenticate(req: &Request, ctx: &Ctx) -> std::result::Result<(), (u16, String)> {
    let header = req
        .header("authorization")
        .ok_or_else(|| (401u16, "missing Authorization header".to_string()))?;

    let presented = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or_else(|| (401u16, "Authorization must be a Bearer token".to_string()))?
        .trim();

    let pairing = ctx
        .pairing
        .lock()
        .map_err(|_| (500u16, "state poisoned".to_string()))?;

    if pairing.verify(presented) {
        Ok(())
    } else {
        Err((401, "unrecognised install secret".into()))
    }
}

/// Pull `probe` out of a query string without pulling in a URL parser.
///
/// Percent-decoding is applied because a script may send a value that needed
/// escaping; the cap keeps a hostile query from echoing something enormous.
fn probe_value(query: &str) -> String {
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("probe=") {
            return percent_decode(v).chars().take(128).collect();
        }
    }
    String::new()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Start a bridge backed by an in-memory library, on an OS-chosen port.
    fn start_test_bridge() -> (Bridge, Arc<Mutex<Pairing>>, Arc<Mutex<Vec<BridgeEvent>>>) {
        start_test_bridge_with(|_, _| Ok(ImportSummary::default()))
    }

    fn start_test_bridge_with<F>(ingest: F) -> (Bridge, Arc<Mutex<Pairing>>, Arc<Mutex<Vec<BridgeEvent>>>)
    where
        F: Fn(&str, i64) -> Result<ImportSummary> + Send + Sync + 'static,
    {
        let pairing = Arc::new(Mutex::new(Pairing::generate().unwrap()));
        let events: Arc<Mutex<Vec<BridgeEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let notify: Notify = Arc::new(move |e| sink.lock().unwrap().push(e));

        // Port 0 lets the OS pick, so tests never collide with a running app.
        let bridge = Bridge::start(
            Arc::clone(&pairing),
            Arc::new(ingest),
            notify,
            BridgeOptions { preferred_port: 0, fallback: 0 },
        )
        .unwrap();
        (bridge, pairing, events)
    }

    /// Send a raw request and return (status, body).
    fn raw(port: u16, request: &str) -> (u16, String) {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut buf = String::new();
        let _ = s.read_to_string(&mut buf);
        let status = buf
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = buf.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        (status, body)
    }

    fn get(port: u16, path: &str, auth: Option<&str>) -> (u16, String) {
        let auth = auth.map(|a| format!("Authorization: Bearer {a}\r\n")).unwrap_or_default();
        raw(port, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}\r\n"))
    }

    fn post(port: u16, path: &str, body: &str, auth: Option<&str>) -> (u16, String) {
        let auth = auth.map(|a| format!("Authorization: Bearer {a}\r\n")).unwrap_or_default();
        raw(
            port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n{auth}\r\n{body}",
                body.len()
            ),
        )
    }

    #[test]
    fn hello_echoes_the_probe_and_discloses_nothing() {
        let (mut b, _, _) = start_test_bridge();
        let (status, body) = get(b.port(), "/v1/hello?probe=abc123", None);
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["app"], "xitter-dl");
        assert_eq!(v["probe"], "abc123");

        // The app must not volunteer anything the caller did not send.
        let text = v.to_string();
        assert!(!text.contains("secret"), "hello leaked a secret: {text}");
        assert!(!text.contains("version"), "hello leaked a version: {text}");
        assert!(!text.contains("path"), "hello leaked a path: {text}");
        b.stop();
    }

    #[test]
    fn hello_works_without_a_probe_and_without_auth() {
        let (mut b, _, _) = start_test_bridge();
        let (status, body) = get(b.port(), "/v1/hello", None);
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["probe"], "");
        b.stop();
    }

    #[test]
    fn capture_without_a_secret_is_refused() {
        let (mut b, _, _) = start_test_bridge();
        let (status, body) = post(b.port(), "/v1/capture", "{}", None);
        assert_eq!(status, 401);
        assert!(body.contains("error"), "{body}");
        b.stop();
    }

    #[test]
    fn capture_with_the_wrong_secret_is_refused() {
        let (mut b, _, _) = start_test_bridge();
        let (status, _) = post(b.port(), "/v1/capture", "{}", Some(&"0".repeat(64)));
        assert_eq!(status, 401);
        b.stop();
    }

    #[test]
    fn a_refused_request_is_surfaced_to_the_user() {
        // Silently dropping a bad secret would leave the user staring at a
        // script that appears connected and captures nothing.
        let (mut b, _, events) = start_test_bridge();
        post(b.port(), "/v1/capture", "{}", Some("wrong"));
        let seen = events.lock().unwrap();
        assert!(matches!(seen.first(), Some(BridgeEvent::Rejected { .. })), "{seen:?}");
        b.stop();
    }

    #[test]
    fn pairing_exchanges_a_code_for_the_secret() {
        let (mut b, pairing, events) = start_test_bridge();
        let code = pairing.lock().unwrap().show_code(crate::now()).unwrap();

        let body = serde_json::json!({ "code": code, "label": "test script" }).to_string();
        let (status, resp) = post(b.port(), "/v1/pair", &body, None);
        assert_eq!(status, 200, "{resp}");

        let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["secret"].as_str().unwrap(), pairing.lock().unwrap().secret());
        assert_eq!(v["port"].as_u64().unwrap() as u16, b.port());
        assert!(matches!(events.lock().unwrap().first(), Some(BridgeEvent::Paired { .. })));
        b.stop();
    }

    #[test]
    fn pairing_with_a_wrong_code_is_refused() {
        let (mut b, pairing, _) = start_test_bridge();
        pairing.lock().unwrap().show_code(crate::now()).unwrap();
        let body = serde_json::json!({ "code": "WRONG234" }).to_string();
        let (status, _) = post(b.port(), "/v1/pair", &body, None);
        assert_eq!(status, 401);
        b.stop();
    }

    #[test]
    fn pairing_with_no_live_code_is_refused() {
        // The panel is closed, so nothing should be exchangeable.
        let (mut b, _, _) = start_test_bridge();
        let body = serde_json::json!({ "code": "ABCD2345" }).to_string();
        let (status, _) = post(b.port(), "/v1/pair", &body, None);
        assert_eq!(status, 401);
        b.stop();
    }

    #[test]
    fn route_paths_and_methods_are_enforced() {
        let (mut b, _, _) = start_test_bridge();
        let (s404, _) = get(b.port(), "/v1/nope", None);
        assert_eq!(s404, 404);
        let (s405, _) = get(b.port(), "/v1/capture", Some("x"));
        assert_eq!(s405, 405, "GET on a POST-only route must be 405");
        b.stop();
    }

    #[test]
    fn a_trailing_slash_still_routes() {
        let (mut b, _, _) = start_test_bridge();
        let (status, _) = get(b.port(), "/v1/hello/", None);
        assert_eq!(status, 200);
        b.stop();
    }

    #[test]
    fn an_oversize_body_is_rejected_with_413() {
        let (mut b, _, _) = start_test_bridge();
        let request = format!(
            "POST /v1/capture HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
            http::MAX_BODY_BYTES + 1
        );
        let (status, _) = raw(b.port(), &request);
        assert_eq!(status, 413);
        b.stop();
    }

    #[test]
    fn a_request_declaring_its_length_both_ways_is_rejected_with_400() {
        // The smuggling shape. Chunked on its own is fine and is exercised by
        // the http unit tests; this is the genuinely ambiguous case.
        let (mut b, _, _) = start_test_bridge();
        let request = "POST /v1/capture HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\
                       Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let (status, _) = raw(b.port(), request);
        assert_eq!(status, 400);
        b.stop();
    }

    #[test]
    fn a_chunked_capture_is_accepted() {
        // Node's http.request sends chunked whenever Content-Length is omitted.
        // Refusing it produced a 400 the script had no way to explain.
        let seen_text = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&seen_text);
        let (mut b, pairing, _) = start_test_bridge_with(move |text, _now| {
            *sink.lock().unwrap() = text.to_string();
            Ok(ImportSummary::default())
        });
        let secret = pairing.lock().unwrap().secret().to_string();

        let body = "{\"v\":1,\"kind\":\"page\"}";
        let request = format!(
            "POST /v1/capture HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {secret}\r\n\
             Transfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
            body.len()
        );
        let (status, resp) = raw(b.port(), &request);

        assert_eq!(status, 200, "{resp}");
        assert_eq!(&*seen_text.lock().unwrap(), body);
        b.stop();
    }

    #[test]
    fn a_valid_capture_reaches_the_ingest_callback() {
        let seen_text = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&seen_text);
        let (mut b, pairing, events) = start_test_bridge_with(move |text, _now| {
            *sink.lock().unwrap() = text.to_string();
            Ok(ImportSummary::default())
        });

        let secret = pairing.lock().unwrap().secret().to_string();
        let line = r#"{"v":1,"kind":"bookmark","captured_at":1000}"#;
        let (status, resp) = post(b.port(), "/v1/capture", line, Some(&secret));

        assert_eq!(status, 200, "{resp}");
        assert_eq!(&*seen_text.lock().unwrap(), line);
        assert!(matches!(events.lock().unwrap().last(), Some(BridgeEvent::Captured(_))));
        b.stop();
    }

    #[test]
    fn a_failing_import_is_reported_not_swallowed() {
        let (mut b, pairing, _) = start_test_bridge_with(|_, _| {
            Err(Error::invalid("no tweets found in NDJSON file"))
        });
        let secret = pairing.lock().unwrap().secret().to_string();
        let (status, resp) = post(b.port(), "/v1/capture", "{}", Some(&secret));
        assert_eq!(status, 400);
        assert!(resp.contains("no tweets"), "{resp}");
        b.stop();
    }

    #[test]
    fn status_requires_auth_and_confirms_a_good_secret() {
        let (mut b, pairing, _) = start_test_bridge();
        let (s401, _) = get(b.port(), "/v1/status", None);
        assert_eq!(s401, 401);

        let secret = pairing.lock().unwrap().secret().to_string();
        let (s200, body) = get(b.port(), "/v1/status", Some(&secret));
        assert_eq!(s200, 200);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["ok"], true);
        b.stop();
    }

    #[test]
    fn the_secret_is_never_echoed_back_anywhere() {
        let (mut b, pairing, _) = start_test_bridge();
        let secret = pairing.lock().unwrap().secret().to_string();

        // Everything reachable without the secret.
        for (status, body) in [
            get(b.port(), "/v1/hello?probe=x", None),
            post(b.port(), "/v1/capture", "{}", None),
            post(b.port(), "/v1/pair", r#"{"code":"WRONG234"}"#, None),
            get(b.port(), "/v1/status", None),
        ] {
            assert!(!body.contains(&secret), "status {status} leaked the secret: {body}");
        }
        b.stop();
    }

    #[test]
    fn port_fallback_moves_off_an_occupied_port() {
        // Occupy a port, then ask the bridge to prefer it.
        let squatter = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = squatter.local_addr().unwrap().port();

        let pairing = Arc::new(Mutex::new(Pairing::generate().unwrap()));
        let notify: Notify = Arc::new(|_| {});
        let ingest: Ingest = Arc::new(|_, _| Ok(ImportSummary::default()));

        let mut b = Bridge::start(
            pairing,
            ingest,
            notify,
            BridgeOptions { preferred_port: taken, fallback: 5 },
        )
        .unwrap();

        assert_ne!(b.port(), taken, "must not have taken an occupied port");
        assert!(b.port() > taken, "should have moved forward, not backward");
        b.stop();
    }

    #[test]
    fn percent_encoded_probes_round_trip() {
        assert_eq!(probe_value("probe=a%20b"), "a b");
        assert_eq!(probe_value("x=1&probe=zz&y=2"), "zz");
        assert_eq!(probe_value("nothing=here"), "");
        // A mangled escape must not panic or truncate the rest.
        assert_eq!(probe_value("probe=%zz"), "%zz");
    }

    #[test]
    fn a_probe_is_capped_so_it_cannot_be_used_to_echo_something_huge() {
        let big = format!("probe={}", "a".repeat(5000));
        assert_eq!(probe_value(&big).len(), 128);
    }
}
