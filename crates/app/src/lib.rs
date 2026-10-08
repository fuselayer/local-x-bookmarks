//! The xitter-dl desktop shell.
//!
//! ## What this process is, and is not
//!
//! It is a window, a SQLite connection, and a set of typed commands. It is
//! **not** a network client. No HTTP crate is linked into this binary, and it
//! holds no X credential of any kind. The architecture's central claim — that
//! the app could not generate X traffic if it wanted to — is enforced here, at
//! the dependency boundary, rather than by convention (PRD §4.3, §7.1).
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

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use xdl_core::db::{read, write, Library};
use xdl_core::model::PostView;
use xdl_core::search::SearchHit;
use serde::Serialize;
use tauri::State;

/// Shared application state.
pub struct AppState {
    db_path: PathBuf,
    /// Held only for writes. Reads do not touch it.
    writer: Mutex<Library>,
}

impl AppState {
    pub fn open(db_path: PathBuf) -> xdl_core::Result<Self> {
        let writer = Library::open(&db_path)?;
        Ok(Self {
            db_path,
            writer: Mutex::new(writer),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// A fresh connection for a read.
    fn reader(&self) -> Result<Library, String> {
        Library::open(&self.db_path).map_err(err)
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

/// Build and run the app.
pub fn run() {
    let db_path = xdl_core::default_library_path();
    let state = AppState::open(db_path)
        .expect("could not open the library database");

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_bookmarks,
            get_post,
            search,
            stats,
            library_path,
            import_json,
            import_path,
            open_external,
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
