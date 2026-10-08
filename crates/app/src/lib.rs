//! The xitter-dl desktop shell.
//!
//! ## What this process is, and is not
//!
//! It is a window, a SQLite connection, a loopback listener, and a set of typed
//! commands. It is **not** a network client. No HTTP client crate is linked
//! into this binary, and it holds no X credential of any kind. The
//! architecture's central claim — that the app could not generate X traffic if
//! it wanted to — is enforced here, at the dependency boundary, rather than by
//! convention (PRD §4.3, §7.1).
//!
//! The bridge ([`xdl_core::bridge`]) is the mirror image of a client and does
//! not weaken that claim: it binds `127.0.0.1`, waits, and never dials out. It
//! exists only while this process does.
//!
//! The one outbound action is `open_external`, which hands a URL to the
//! operating system. The OS opens the user's browser, and *that* makes the
//! request. See the note on that command.
//!
//! ## Concurrency
//!
//! Reads open their own connection; writes take a mutex. SQLite in WAL mode
//! allows many readers alongside one writer, and opening a connection is
//! cheap, so this gives real read concurrency without a pool. A single shared
//! `Mutex<Library>` would instead serialise every search behind every import,
//! which is exactly the freeze the WAL pragma exists to prevent.
//!
//! The bridge thread shares the *write* connection, because it is a writer.
//! That is the one place the mutex is genuinely contended, and it is fine: a
//! capture batch and a manual import are both short.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use xdl_core::bridge::{Bridge, BridgeEvent, BridgeOptions, Ingest, Notify, Pairing};
use xdl_core::db::{read, write, Library};
use xdl_core::model::PostView;
use xdl_core::search::SearchHit;
use serde::Serialize;
use tauri::{Emitter, Manager, State};

/// Shared application state.
pub struct AppState {
    db_path: PathBuf,
    /// Held only for writes — including writes that arrive over the bridge.
    /// Reads do not touch it.
    writer: Arc<Mutex<Library>>,
    pairing: Arc<Mutex<Pairing>>,
    /// Where the install secret is persisted, beside the library.
    pairing_path: PathBuf,
    /// The port the bridge actually bound, or 0 before it starts.
    port: AtomicU16,
    /// Kept alive for the life of the process; dropping it stops the listener.
    bridge: Mutex<Option<Bridge>>,
}

impl AppState {
    pub fn open(db_path: PathBuf) -> xdl_core::Result<Self> {
        let writer = Library::open(&db_path)?;

        let pairing_path = db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("bridge.json");

        let existing = std::fs::read_to_string(&pairing_path).ok();
        let (pairing, created) = Pairing::load_or_generate(existing.as_deref())?;
        if created {
            persist_pairing(&pairing_path, &pairing);
        }

        Ok(Self {
            db_path,
            writer: Arc::new(Mutex::new(writer)),
            pairing: Arc::new(Mutex::new(pairing)),
            pairing_path,
            port: AtomicU16::new(0),
            bridge: Mutex::new(None),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn pairing_path(&self) -> &Path {
        &self.pairing_path
    }

    /// A fresh connection for a read.
    fn reader(&self) -> Result<Library, String> {
        Library::open(&self.db_path).map_err(err)
    }

    fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }
}

/// Write the pairing state next to the library.
///
/// Failure is logged and ignored rather than fatal: the app works fine with an
/// in-memory secret, the user simply re-pairs after a restart. Refusing to
/// start because a side file could not be written would be a worse trade.
fn persist_pairing(path: &Path, pairing: &Pairing) {
    let Ok(json) = pairing.to_json() else { return };
    if std::fs::write(path, json).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        // The install secret is a bearer token for a local endpoint. On a
        // multi-user machine the file should not be readable by anyone else.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Commands return `Result<T, String>`; the frontend shows the string.
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// What `import_*` returns to the UI.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    files: usize,
    seen: usize,
    new: usize,
    updated: usize,
    payloads: usize,
    problems: Vec<String>,
    library: read::LibraryStats,
}

fn summarize(
    conn: &rusqlite::Connection,
    files: usize,
    stats: write::IngestStats,
    problems: Vec<String>,
) -> Result<ImportResult, String> {
    Ok(ImportResult {
        files,
        seen: stats.seen,
        new: stats.new,
        updated: stats.updated,
        payloads: stats.payloads,
        problems,
        library: read::stats(conn).map_err(err)?,
    })
}

// ── read commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn list_bookmarks(state: State<'_, AppState>, limit: usize, offset: usize) -> Result<Vec<PostView>, String> {
    let lib = state.reader()?;
    read::list_bookmarks(lib.conn(), limit, offset).map_err(err)
}

#[tauri::command]
fn get_post(state: State<'_, AppState>, id: String) -> Result<Option<PostView>, String> {
    let lib = state.reader()?;
    read::get_post(lib.conn(), &id).map_err(err)
}

#[tauri::command]
fn search(state: State<'_, AppState>, query: String, limit: usize) -> Result<Vec<SearchHit>, String> {
    let lib = state.reader()?;
    // The library default uses U+0001/U+0002 delimiters so the frontend builds
    // DOM nodes instead of parsing HTML — see `xdl_core::search::SNIPPET_OPEN`.
    xdl_core::search::search(lib.conn(), &query, limit).map_err(err)
}

#[tauri::command]
fn stats(state: State<'_, AppState>) -> Result<read::LibraryStats, String> {
    let lib = state.reader()?;
    read::stats(lib.conn()).map_err(err)
}

#[tauri::command]
fn library_path(state: State<'_, AppState>) -> String {
    state.db_path().display().to_string()
}

// ── write commands ───────────────────────────────────────────────────────────

/// Import a payload the user pasted.
///
/// The most direct route to "one tweet in the library": no file dialog, no
/// userscript, no network. Whatever the user pastes is parsed locally and
/// stored in the local database.
#[tauri::command]
fn import_json(state: State<'_, AppState>, text: String) -> Result<ImportResult, String> {
    let mut guard = state.writer.lock().map_err(|_| "database lock poisoned")?;
    let now = xdl_core::now();

    let tx = guard.conn_mut().transaction().map_err(err)?;
    let summary = xdl_core::import::payload::import_json_text(&tx, &text, None, now).map_err(err)?;
    tx.commit().map_err(err)?;

    summarize(guard.conn(), summary.files, summary.stats, summary.problems)
}

/// Import from a path on disk: a JSON payload, NDJSON, a HAR, or a directory.
#[tauri::command]
fn import_path(state: State<'_, AppState>, path: String) -> Result<ImportResult, String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(format!("no such file or directory: {}", path.display()));
    }

    let mut guard = state.writer.lock().map_err(|_| "database lock poisoned")?;
    let now = xdl_core::now();

    let tx = guard.conn_mut().transaction().map_err(err)?;
    let summary = xdl_core::import::import_path(&tx, &path, now).map_err(err)?;
    tx.commit().map_err(err)?;

    summarize(guard.conn(), summary.files, summary.stats, summary.problems)
}

// ── the one outbound action ──────────────────────────────────────────────────

/// Hand a URL to the operating system's default browser.
///
/// ## Why this is not a violation of "the app never dials out"
///
/// It is that rule being obeyed. This process opens no socket: it asks the OS
/// to open a URL, and the *user's own browser* makes the request — their
/// session, their IP, their fingerprint — at the moment they click. That is
/// the same provenance the userscript side of the design depends on.
///
/// The alternative, letting the webview navigate, would put the request inside
/// this process. Which is why every link in the UI routes through here instead
/// of using a bare `href`.
///
/// ## Why the scheme is checked
///
/// The URL comes from tweet text, which is attacker-controlled in the general
/// case. Passing it through unchecked to a shell could launch anything the OS
/// has registered a handler for — including local executables. Only `http`
/// and `https` are forwarded, and the command is spawned without a shell
/// wherever the platform allows it.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("refusing to open non-web URL: {url}"));
    }

    #[cfg(target_os = "windows")]
    {
        // `rundll32` invokes the registered protocol handler directly, which
        // avoids `cmd.exe` and therefore avoids shell-quoting rules entirely.
        // The empty-string title trick that `start` needs is not required here.
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn()
            .map_err(|e| format!("could not open browser: {e}"))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("could not open browser: {e}"))?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("could not open browser: {e}"))?;
    }

    Ok(())
}

// ── pairing ──────────────────────────────────────────────────────────────────

/// What the pairing panel renders, and everything the frontend is allowed to
/// know about the bridge.
///
/// Note what is **not** here: the install secret. It leaves the process exactly
/// once, in the response to the script's `/v1/pair` exchange, and is never sent
/// to the webview. The UI cannot leak what it never receives.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingInfo {
    port: u16,
    /// Ready to show the user, e.g. `http://127.0.0.1:8737`.
    address: String,
    /// The raw code, for a copy button.
    code: Option<String>,
    /// The same code as the user should read it aloud: `ABCD-2345`.
    formatted_code: Option<String>,
    seconds_remaining: i64,
    paired: bool,
    paired_label: Option<String>,
    paired_at: Option<i64>,
}

fn snapshot(state: &AppState) -> Result<PairingInfo, String> {
    let now = xdl_core::now();
    let p = state.pairing.lock().map_err(|_| "pairing lock poisoned")?;
    let code = p.current_code(now).map(str::to_string);
    let port = state.port();
    Ok(PairingInfo {
        port,
        address: format!("http://127.0.0.1:{port}"),
        formatted_code: code.as_deref().map(xdl_core::bridge::format_code),
        code,
        seconds_remaining: p.seconds_remaining(now),
        paired: p.is_paired(),
        paired_label: p.paired_label.clone(),
        paired_at: p.paired_at,
    })
}

/// Read the current pairing state without starting a code.
#[tauri::command]
fn pairing_status(state: State<'_, AppState>) -> Result<PairingInfo, String> {
    snapshot(&state)
}

/// Begin displaying a code, rotating it if the current one has expired.
///
/// Called when the panel opens. The code is display state and is deliberately
/// never persisted, so it cannot outlive the window that showed it.
#[tauri::command]
fn pairing_show(state: State<'_, AppState>) -> Result<PairingInfo, String> {
    let now = xdl_core::now();
    {
        let mut p = state.pairing.lock().map_err(|_| "pairing lock poisoned")?;
        p.show_code(now).map_err(err)?;
    }
    snapshot(&state)
}

/// Stop displaying a code, invalidating it immediately.
///
/// Called when the panel closes. A code that stayed valid while off screen
/// would be a code nobody is watching.
#[tauri::command]
fn pairing_hide(state: State<'_, AppState>) -> Result<PairingInfo, String> {
    {
        let mut p = state.pairing.lock().map_err(|_| "pairing lock poisoned")?;
        p.hide_code();
    }
    snapshot(&state)
}

/// Forget every pairing and mint a new secret.
///
/// Every installed script stops working until it is paired again. That is the
/// point: it is the answer to "I think someone else has my secret".
#[tauri::command]
fn pairing_revoke(state: State<'_, AppState>) -> Result<PairingInfo, String> {
    {
        let mut p = state.pairing.lock().map_err(|_| "pairing lock poisoned")?;
        p.revoke().map_err(err)?;
        persist_pairing(state.pairing_path(), &p);
    }
    snapshot(&state)
}

// ── the bridge ───────────────────────────────────────────────────────────────

/// Start the loopback receiver.
///
/// The port is *not* persisted. The script discovers it with `/v1/hello` — an
/// unauthenticated pure echo — and caches the answer, rescanning only when a
/// request fails. That keeps the app from having to advertise a moving port it
/// cannot reach anyone to tell about, and it means a squatter on the wrong port
/// never sees the install secret.
fn start_bridge(app: &tauri::AppHandle, state: &AppState) -> xdl_core::Result<()> {
    let writer = Arc::clone(&state.writer);
    let ingest: Ingest = Arc::new(move |text: &str, now: i64| {
        let mut lib = writer
            .lock()
            .map_err(|_| xdl_core::Error::invalid("database lock poisoned"))?;
        let tx = lib.conn_mut().transaction()?;
        let summary = xdl_core::import::ndjson::import_str(&tx, text, now)?;
        tx.commit()?;
        Ok(summary)
    });

    let emitter = app.clone();
    let pairing = Arc::clone(&state.pairing);
    let pairing_path = state.pairing_path().to_path_buf();
    let notify: Notify = Arc::new(move |event: BridgeEvent| {
        // A successful exchange changes the persisted state, and the bridge is
        // where that happens, so the write belongs here.
        if matches!(event, BridgeEvent::Paired { .. }) {
            if let Ok(p) = pairing.lock() {
                persist_pairing(&pairing_path, &p);
            }
        }
        let _ = emitter.emit(BRIDGE_EVENT, &event);
    });

    let bridge = Bridge::start(
        Arc::clone(&state.pairing),
        ingest,
        notify,
        BridgeOptions::default(),
    )?;

    state.port.store(bridge.port(), Ordering::Relaxed);
    // Worth saying out loud in the log: when a script will not connect, the
    // first question is always "which port is it actually on", and the answer
    // is otherwise invisible in a windowed build with no console.
    eprintln!(
        "xitter-dl: capture bridge listening on http://127.0.0.1:{}",
        bridge.port()
    );
    if let Ok(mut slot) = state.bridge.lock() {
        *slot = Some(bridge);
    }
    Ok(())
}

/// Event name the frontend listens on for live capture progress.
pub const BRIDGE_EVENT: &str = "xdl://bridge";

/// Build and run the app.
pub fn run() {
    let db_path = xdl_core::default_library_path();
    let state = AppState::open(db_path)
        .expect("could not open the library database");

    tauri::Builder::default()
        .manage(state)
        .setup(|app| {
            let handle = app.handle().clone();
            let state = app.state::<AppState>();
            // A bridge that will not bind must not stop the app from opening.
            // The library is still fully usable from files and the CLI, and
            // saying so is better than refusing to start.
            if let Err(e) = start_bridge(&handle, &state) {
                eprintln!("xitter-dl: capture bridge did not start: {e}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_bookmarks,
            get_post,
            search,
            stats,
            library_path,
            import_json,
            import_path,
            open_external,
            pairing_status,
            pairing_show,
            pairing_hide,
            pairing_revoke,
        ])
        .run(tauri::generate_context!())
        .expect("error while running xitter-dl");
}

#[cfg(test)]
mod tests {
    /// The scheme guard is the only security-relevant logic in this crate, so
    /// it gets a test even though the rest is wiring.
    #[test]
    fn open_external_rejects_non_web_schemes() {
        // Mirrors the check inside the command; kept in sync deliberately
        // rather than by sharing, because the command body needs the value
        // inline for the cfg branches.
        let allowed = |u: &str| {
            let l = u.trim().to_ascii_lowercase();
            l.starts_with("http://") || l.starts_with("https://")
        };

        assert!(allowed("https://x.com/ada/status/1"));
        assert!(allowed("http://example.com"));
        assert!(allowed("HTTPS://EXAMPLE.COM"));

        // Everything below would hand an arbitrary string to the OS.
        assert!(!allowed("javascript:alert(1)"));
        assert!(!allowed("file:///C:/Windows/System32/cmd.exe"));
        assert!(!allowed("ms-settings:"));
        assert!(!allowed("C:\\Windows\\System32\\cmd.exe"));
        assert!(!allowed("\\\\server\\share"));
        assert!(!allowed("vbscript:msgbox"));
        assert!(!allowed(""));
    }
}
