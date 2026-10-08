//! The domain model, and the contract with the frontend.
//!
//! ## Why entities carry indices instead of pre-rendered HTML
//!
//! X renders a tweet by slicing `full_text` at entity indices and wrapping the
//! slices in links. We could flatten that to HTML in Rust and send a string,
//! but then the frontend cannot restyle, cannot re-highlight search hits
//! inside a link, and cannot sanitise. So we send the text plus the raw
//! indices and let the renderer compose — the same thing X's own client does.
//!
//! ## A note on index units, which is a real trap
//!
//! X's indices are documented for the public API as Unicode code points, but
//! the web client slices them with JavaScript `String.slice`, which counts
//! UTF-16 code units. For any tweet containing an emoji or other astral-plane
//! character these disagree, and the wrong choice silently misplaces links by
//! one position per astral character.
//!
//! We do not guess. `EntityIndexUnits` records which convention the parser
//! believed, and the renderer *verifies* each slice by checking that
//! `text.slice(start, end)` actually equals the entity's own URL or tag
//! before styling it. A mismatch degrades to plain text rather than
//! corrupting the tweet. See `[Unverified — validate against live payloads]`
//! in the PRD; this is how we stay correct without that validation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntityIndexUnits {
    /// Indices count UTF-16 code units, matching JavaScript string indexing.
    Utf16,
    /// Indices count Unicode scalar values.
    CodePoints,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entities {
    pub units: Option<EntityIndexUnits>,
    pub urls: Vec<UrlEntity>,
    pub mentions: Vec<MentionEntity>,
    pub hashtags: Vec<TagEntity>,
    pub symbols: Vec<TagEntity>,
    /// `legacy.extended_entities.media` count, used to decide whether to show
    /// a media grid. The media themselves live in `Tweet::media`.
    pub media_indices: Vec<(usize, usize)>,
}

impl Entities {
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty()
            && self.mentions.is_empty()
            && self.hashtags.is_empty()
            && self.symbols.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlEntity {
    /// The `t.co` link as it appears in the text.
    pub url: String,
    /// What X displays: `github.com/foo/bar`.
    pub display: String,
    /// The real destination, straight from the payload.
    ///
    /// This is the field that lets us avoid unfurling `t.co` ourselves — a
    /// HEAD request to `t.co` would be an app-originated request to an
    /// X-owned host, which PRD §7.1 forbids and §7.7 spells out.
    pub expanded: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionEntity {
    pub handle: String,
    pub name: Option<String>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagEntity {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// Everything X needs to draw the author line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    pub id: String,
    pub handle: String,
    pub name: String,
    pub avatar_url: Option<String>,
    /// Legacy verified (the pre-2023 checkmark).
    pub verified: bool,
    /// X Premium blue check. X renders these differently — `verified` gets the
    /// old badge, `blue_verified` the current one — so they are kept apart.
    pub blue_verified: bool,
    /// `blue` | `business` | `government` | `none`, when X supplies it.
    pub verified_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Photo,
    Video,
    Gif,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub kind: MediaKind,
    /// Best still image available in the payload.
    ///
    /// Note `?name=orig` is *not* appended. Reaching for a larger variant than
    /// the page rendered is a new request to `pbs.twimg.com` originated by us,
    /// which PRD §7.5 scopes out of the default path. The URL is stored
    /// exactly as the payload gave it.
    pub url: String,
    pub alt_text: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// Progressive MP4 for video/GIF, highest bitrate variant in the payload.
    pub video_url: Option<String>,
    pub duration_ms: Option<i64>,
}

impl Media {
    /// Width divided by height, for reserving layout space before the image
    /// loads. X does this too — it is why its timeline does not jump.
    pub fn aspect_ratio(&self) -> Option<f64> {
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some(w as f64 / h as f64),
            _ => None,
        }
    }
}

/// A link preview card, straight from the payload's `card` object.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    /// X's card type, e.g. `summary_large_image`, `summary`, `player`.
    pub name: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// The destination URL. X shows this as the small footer text.
    pub url: Option<String>,
    /// Display domain, derived from `url` — X shows `github.com`, not the path.
    pub domain: Option<String>,
    pub image_url: Option<String>,
    pub image_width: Option<i64>,
    pub image_height: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub likes: i64,
    pub reposts: i64,
    pub replies: i64,
    pub quotes: i64,
    pub bookmarks: i64,
    pub views: Option<i64>,
}

/// A post as the UI needs it, derived from `posts.raw_tweet` at read time.
///
/// Deriving rather than storing the parsed form is the PRD §9.2 strategy:
/// when X changes shape and we ship a new parser, every stored payload is
/// re-interpreted retroactively with no re-capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tweet {
    pub id: String,
    pub author: Author,
    /// The text to display: `note_tweet` when present, else `full_text`.
    pub text: String,
    /// The `full_text`, when it differs from `text` because this is a long post.
    pub truncated_text: Option<String>,
    pub is_long_form: bool,
    pub entities: Entities,
    /// Unix seconds. `None` if X sent a timestamp we could not read.
    pub created_at: Option<i64>,
    pub lang: Option<String>,
    pub conversation_id: Option<String>,
    pub in_reply_to_id: Option<String>,
    pub in_reply_to_handle: Option<String>,
    pub metrics: Metrics,
    pub media: Vec<Media>,
    pub card: Option<Card>,
    /// The quoted post, when this tweet quotes one.
    pub quoted: Option<Box<Tweet>>,
    /// Which parser version produced this. Stored with the row so a future
    /// re-parse can find records that predate a fix.
    pub parser_version: u32,
}

impl Tweet {
    /// What X shows under the avatar: `@handle`.
    pub fn handle(&self) -> String {
        format!("@{}", self.author.handle)
    }
}

/// A `Tweet` plus the local, user-specific state we layer on top.
///
/// Kept as a separate type from `Tweet` so that everything derived purely
/// from the X payload stays independently testable, with no database in
/// scope. `#[serde(flatten)]` means the frontend sees one flat object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostView {
    #[serde(flatten)]
    pub tweet: Tweet,

    /// When *we* observed the save.
    ///
    /// Null for anything captured from a backfill scroll, because X does not
    /// expose a bookmark timestamp anywhere in the payload (PRD §6.3). Only
    /// continuous capture can fill this in, having watched the save happen.
    /// The UI must label it "saved (observed)" and must never sort by it.
    pub bookmarked_at: Option<i64>,

    /// `entries[].sortIndex` — the entry-level ordering key. See the warning
    /// in `x::tweet` about this not living under `legacy`.
    pub sort_index: Option<String>,

    pub first_seen_at: i64,
    pub last_seen_at: Option<i64>,

    /// Set when a later capture covered this post's sort-index range and did
    /// not find it. The badge reads "no longer in your bookmarks" — never
    /// "deleted", because absence cannot distinguish deletion from an
    /// unbookmark, from filtering, or from a partial capture.
    pub removed_at: Option<i64>,

    /// `capture` | `file_import`
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_aspect_ratio_is_none_rather_than_dividing_by_zero() {
        let mut m = Media {
            kind: MediaKind::Photo,
            url: "u".into(),
            alt_text: None,
            width: Some(0),
            height: Some(100),
            video_url: None,
            duration_ms: None,
        };
        assert_eq!(m.aspect_ratio(), None);

        m.width = Some(1200);
        m.height = Some(675);
        assert_eq!(m.aspect_ratio(), Some(1200.0 / 675.0));

        m.height = None;
        assert_eq!(m.aspect_ratio(), None);
    }
}
