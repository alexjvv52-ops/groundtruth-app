//! LAN HTTP port. S1 + S4a(a) + S4b(a) + S6, D1(b) + D3(b), signed 2026-08-21.
//! The port is a READ door. The desk stays the sole writer. Nothing here
//! recomputes severity, rank or grouping.
//!
//! Auth reuses the existing Admin token (`field_devices::resolve_token`).
//! Only `DeviceResolution::Live` passes. Production binds `DOCK_BIND`.
//! Tests pass a loopback ephemeral bind into the same `start`. The listener
//! does not start itself.
//!
//! J5 DOCK-ACCEPT (signed 2026-09-07): every accepted connection runs on its
//! own thread under one whole-request deadline; the accept thread only
//! accepts, and `stop()` joins that thread alone.

use crate::field_devices::{self, DeviceResolution};
use rusqlite::Connection;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const HEADER_CAP: usize = 8192;
/// J5 DEADLINE A: one connection may hold its handler for this long, first
/// byte to last write. Monotonic, never a wall-clock read.
const REQUEST_DEADLINE: Duration = Duration::from_secs(8);
/// One read waits this long at most, and never past the deadline.
const READ_STEP: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const READY_BODY: &str = r#"{"ready":true}"#;
const BAD_REQUEST: &str = r#"{"error":"Bad request."}"#;
const FORBIDDEN: &str = r#"{"error":"Forbidden."}"#;
const UNAUTHORIZED: &str = r#"{"error":"Unauthorized."}"#;
const READ_ONLY: &str = r#"{"error":"Read only."}"#;
const NOT_FOUND: &str = r#"{"error":"Not found."}"#;

pub const DOCK_BIND: &str = "0.0.0.0:18765";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockPortView {
    pub running: bool,
    pub port: Option<u16>,
    pub reach_url: Option<String>,
    pub reach_qr: Option<Vec<Vec<bool>>>,
}

struct Running {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

static PORT: Mutex<Option<Running>> = Mutex::new(None);

/// Bind the given address. Idempotent: a running port is returned, not rebound.
pub fn start(
    db: Arc<Mutex<Connection>>,
    farm_dir: PathBuf,
    snapshots_dir: PathBuf,
    bind: &str,
) -> Result<DockPortView, String> {
    start_with(db, farm_dir, snapshots_dir, bind, REQUEST_DEADLINE)
}

/// J5 DOCK-ACCEPT: the same door with the whole-request deadline passed in.
/// Production reaches it through `start` with `REQUEST_DEADLINE`; tests pass
/// a short one the way they pass a loopback bind.
pub(crate) fn start_with(
    db: Arc<Mutex<Connection>>,
    farm_dir: PathBuf,
    snapshots_dir: PathBuf,
    bind: &str,
    deadline: Duration,
) -> Result<DockPortView, String> {
    let mut slot = PORT.lock().map_err(|e| e.to_string())?;
    if let Some(running) = slot.as_ref() {
        return Ok(port_view(true, Some(running.port)));
    }
    let listener = TcpListener::bind(bind).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread =
        thread::spawn(move || accept_loop(listener, flag, db, farm_dir, snapshots_dir, deadline));
    *slot = Some(Running {
        port,
        stop,
        thread: Some(thread),
    });
    Ok(port_view(true, Some(port)))
}

/// J5 STOP A: sets the flag and joins the accept thread only. That thread
/// owns the listener and drops it on the way out, so a connect after `stop`
/// fails; an in-flight handler is detached and ends by its own deadline.
pub fn stop() -> DockPortView {
    let running = match PORT.lock() {
        Ok(mut slot) => slot.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    if let Some(mut running) = running {
        running.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = running.thread.take() {
            let _ = thread.join();
        }
    }
    port_view(false, None)
}

pub fn status() -> DockPortView {
    match PORT.lock() {
        Ok(slot) => match slot.as_ref() {
            Some(running) => port_view(true, Some(running.port)),
            None => port_view(false, None),
        },
        Err(_) => port_view(false, None),
    }
}

fn port_view(running: bool, port: Option<u16>) -> DockPortView {
    let reach_url = lan_reach_url(port);
    let reach_qr = reach_url
        .as_deref()
        .and_then(|url| crate::wholesale::qr_modules(url).ok());
    DockPortView {
        running,
        port,
        reach_url,
        reach_qr,
    }
}

fn lan_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?;
    Some(sock.local_addr().ok()?.ip())
}

fn lan_reach_url(port: Option<u16>) -> Option<String> {
    let ip = lan_ipv4()?;
    let p = port.unwrap_or(18765);
    Some(format!("http://{ip}:{p}"))
}

/// J5 THREAD A: the accept thread only accepts. Each connection runs on its
/// own thread with its own deadline, so a slow client holds one handler —
/// never the door, never `stop()`.
fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    db: Arc<Mutex<Connection>>,
    farm_dir: PathBuf,
    snapshots_dir: PathBuf,
    deadline: Duration,
) {
    let farm_dir: Arc<Path> = Arc::from(farm_dir);
    let snapshots_dir: Arc<Path> = Arc::from(snapshots_dir);
    loop {
        // The flag is read first, every turn, so a run of successful accepts
        // can never starve it.
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let db = Arc::clone(&db);
                let farm_dir = Arc::clone(&farm_dir);
                let snapshots_dir = Arc::clone(&snapshots_dir);
                // Detached on purpose (STOP A): the handle is dropped, the
                // handler ends by its own deadline or the client's close.
                thread::spawn(move || {
                    handle_stream(stream, &db, &farm_dir, &snapshots_dir, deadline)
                });
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn handle_stream(
    mut stream: TcpStream,
    db: &Arc<Mutex<Connection>>,
    farm_dir: &Path,
    snapshots_dir: &Path,
    deadline: Duration,
) {
    // DEADLINE A: one monotonic instant for the whole request. The mutex wait
    // in `with_conn` sits outside it on purpose — the desk's single writer
    // is never cut short by a phone.
    let due = Instant::now() + deadline;
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let headers = match read_headers(&mut stream, due) {
        Ok(h) => h,
        Err(()) => {
            write_response(&mut stream, 400, BAD_REQUEST, false);
            return;
        }
    };
    let req = match parse_request(&headers) {
        Ok(r) => r,
        Err(Refuse::BadRequest) => {
            write_response(&mut stream, 400, BAD_REQUEST, false);
            return;
        }
        Err(Refuse::Forbidden) => {
            write_response(&mut stream, 403, FORBIDDEN, false);
            return;
        }
        Err(Refuse::ReadOnly) => {
            write_response(&mut stream, 405, READ_ONLY, false);
            return;
        }
    };
    let head = req.method == "HEAD";
    if req.path == "/" {
        write_html(&mut stream, 200, crate::dock_shell::SHELL, head);
        return;
    }
    match with_conn(db, |conn| decide(conn, &req, farm_dir, snapshots_dir)) {
        Ok((code, body)) => write_response(&mut stream, code, &body, head),
        Err(()) => write_response(&mut stream, 400, BAD_REQUEST, head),
    }
}

fn with_conn<R>(db: &Arc<Mutex<Connection>>, f: impl FnOnce(&Connection) -> R) -> Result<R, ()> {
    let guard = db.lock().map_err(|_| ())?;
    Ok(f(&guard))
}

/// Token check and route. `conn` is borrowed read-only.
fn decide(conn: &Connection, req: &Parsed, farm_dir: &Path, snapshots_dir: &Path) -> (u16, String) {
    if !token_is_live(conn, req.token.as_deref()) {
        return (401, UNAUTHORIZED.to_string());
    }
    if req.path == "/folds" {
        // FI-1 - one wall-clock read. servedAt and the day boundary are derived
        // from the same instant so they cannot straddle midnight.
        let now = crate::db::utc_now_rfc3339();
        let Some(today) = crate::dock_folds::local_date_of(&now) else {
            return (400, BAD_REQUEST.to_string());
        };
        match crate::dock_folds::port_document(conn, farm_dir, snapshots_dir, &now, today)
            .and_then(|doc| serde_json::to_string(&doc).map_err(|e| e.to_string()))
        {
            Ok(body) => return (200, body),
            Err(_) => return (400, BAD_REQUEST.to_string()),
        }
    }
    if req.path != "/ready" {
        return (404, NOT_FOUND.to_string());
    }
    (200, READY_BODY.to_string())
}

fn token_is_live(conn: &Connection, token: Option<&str>) -> bool {
    let Some(token) = token else {
        return false;
    };
    matches!(
        field_devices::resolve_token(conn, token),
        Ok(DeviceResolution::Live(_))
    )
}

enum Refuse {
    BadRequest,
    Forbidden,
    ReadOnly,
}

struct Parsed {
    method: String,
    path: String,
    token: Option<String>,
}

fn read_headers(stream: &mut TcpStream, due: Instant) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        if buf.len() >= HEADER_CAP {
            return Err(());
        }
        // DEADLINE A: each read waits READ_STEP at most and never past `due`,
        // so one byte per read cannot stretch a request past the deadline.
        let remaining = due.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(());
        }
        stream
            .set_read_timeout(Some(remaining.min(READ_STEP)))
            .map_err(|_| ())?;
        let room = HEADER_CAP - buf.len();
        let nread = tmp.len().min(room);
        let n = stream.read(&mut tmp[..nread]).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if find_header_end(&buf).is_some() {
            return Ok(buf);
        }
        if buf.len() >= HEADER_CAP {
            return Err(());
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn parse_request(raw: &[u8]) -> Result<Parsed, Refuse> {
    let end = find_header_end(raw).ok_or(Refuse::BadRequest)?;
    let text = std::str::from_utf8(&raw[..end]).map_err(|_| Refuse::BadRequest)?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(Refuse::BadRequest)?;
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or(Refuse::BadRequest)?;
    let target = parts.next().ok_or(Refuse::BadRequest)?;
    let version = parts.next().ok_or(Refuse::BadRequest)?;
    if parts.next().is_some() || method.is_empty() || target.is_empty() {
        return Err(Refuse::BadRequest);
    }
    if version != "HTTP/1.0" && version != "HTTP/1.1" {
        return Err(Refuse::BadRequest);
    }
    let path = target.split('?').next().unwrap_or(target).to_string();

    let mut host: Option<String> = None;
    let mut token: Option<String> = None;
    let mut has_body_header = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(Refuse::BadRequest)?;
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("Content-Length")
            || name.eq_ignore_ascii_case("Transfer-Encoding")
        {
            has_body_header = true;
        } else if name.eq_ignore_ascii_case("Host") && host.is_none() {
            host = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("X-Dock-Token") && token.is_none() {
            token = Some(value.to_string());
        }
    }
    if has_body_header {
        return Err(Refuse::BadRequest);
    }
    match host.as_deref() {
        Some(h) if host_is_addressable(h) => {}
        _ => return Err(Refuse::Forbidden),
    }
    if method != "GET" && method != "HEAD" {
        return Err(Refuse::ReadOnly);
    }
    Ok(Parsed {
        method: method.to_string(),
        path,
        token,
    })
}

fn host_name(host: &str) -> &str {
    let h = host.trim();
    if let Some(rest) = h.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let after = &rest[end + 1..];
            if after.is_empty()
                || (after.starts_with(':')
                    && after.len() > 1
                    && after[1..].chars().all(|c| c.is_ascii_digit()))
            {
                return &h[..end + 2];
            }
        }
    }
    match h.rsplit_once(':') {
        Some((n, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => n,
        _ => h,
    }
}

fn host_is_addressable(host: &str) -> bool {
    let name = host_name(host);
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let literal = name
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(name);
    literal.parse::<IpAddr>().is_ok()
}

fn write_response(stream: &mut TcpStream, code: u16, body: &str, head: bool) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let bytes = body.as_bytes();
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(header.as_bytes());
    if !head {
        let _ = stream.write_all(bytes);
    }
    let _ = stream.flush();
}

fn write_html(stream: &mut TcpStream, code: u16, body: &str, head: bool) {
    let reason = match code {
        200 => "OK",
        _ => "OK",
    };
    let bytes = body.as_bytes();
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(header.as_bytes());
    if !head {
        let _ = stream.write_all(bytes);
    }
    let _ = stream.flush();
}
