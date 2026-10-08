//! The bridge between the companion userscript and the app.
//!
//! ## Direction, and why it is one-way by construction
//!
//! The app **binds loopback and receives**. It never dials out to any X-owned
//! host and never holds an X credential. Every byte of X traffic stays inside
//! the user's own browser, in their own session, behind their own IP — which is
//! the architectural boundary this whole product rests on, not a stylistic
//! preference.
//!
//! ```text
//!   user's browser                    xitter-dl
//!   ┌────────────────────┐            ┌──────────────────────┐
//!   │ x.com + the script │            │  loopback receiver   │
//!   │  fetch/XHR hook    │  NDJSON    │  bearer-auth'd       │
//!   │  GM_xmlhttpRequest ├───────────►│  validate → store    │
//!   └────────────────────┘  127.0.0.1 └──────────────────────┘
//! ```
//!
//! ## Layout
//!
//! - [`http`] — a minimal, deliberately strict HTTP/1.1 reader and writer.
//! - [`pairing`] — the short code, the persistent secret, and the lockout.
//! - [`server`] — routing, authentication, port selection, the accept loop.
//!
//! The three are in `core` rather than in the Tauri crate on purpose: the
//! receiver is the part most worth testing adversarially, and it must be
//! testable without standing up a window.

pub mod http;
pub mod pairing;
pub mod server;

pub use pairing::{format_code, Pairing, CODE_TTL_SECS, LOCKOUT_SECS, MAX_ATTEMPTS};
pub use server::{
    Bridge, BridgeEvent, BridgeOptions, CaptureReport, Ingest, Notify, DEFAULT_PORT, PORT_FALLBACK,
};
