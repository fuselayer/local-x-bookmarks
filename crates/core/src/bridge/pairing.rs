//! Pairing: exchange a short code once, keep a secret forever.
//!
//! ## The design problem
//!
//! The obvious scheme — a random port and a fresh single-use token every
//! launch — forces the user to re-pair constantly, and a secret someone has to
//! keep re-pasting is a secret they eventually stop reading. So the exchange
//! happens once and the result persists.
//!
//! ```text
//!   app shows a code  ──►  user pastes it into the script pill  ──►  done
//!                          (script stores the install secret)
//! ```
//!
//! After that the script presents the secret on every request and the app never
//! asks again. Re-pairing happens when the user reinstalls, clears script
//! storage, or explicitly revokes — not every time the app opens.
//!
//! ## Why the code is short and the secret is long
//!
//! They defend against different things. The **code** is typed by a human, so
//! it is short and lives for [`CODE_TTL_SECS`]; brute-forcing it is defeated by
//! rotation plus the lockout in [`Pairing::redeem`]. The **secret** is never
//! typed, so it is 256 bits of CSPRNG output and defends the endpoint for as
//! long as it exists.
//!
//! ## Comparison
//!
//! Both comparisons are constant-time. Not because a remote timing attack on
//! loopback is likely, but because writing `==` on a secret is the kind of
//! habit that survives into code where it does matter, and the fix is four
//! lines.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How long a displayed pairing code stays valid.
pub const CODE_TTL_SECS: i64 = 120;

/// How many wrong codes before the endpoint stops answering for a while.
pub const MAX_ATTEMPTS: u32 = 5;

/// How long the endpoint stays locked after [`MAX_ATTEMPTS`] wrong codes.
pub const LOCKOUT_SECS: i64 = 60;

/// Secret length in bytes. 256 bits, hex-encoded to 64 characters.
const SECRET_BYTES: usize = 32;

/// Code length in characters, excluding the separator.
const CODE_CHARS: usize = 8;

/// Crockford-style base32: no `0`/`O`, no `1`/`I`/`L`, no `U`.
///
/// The user reads this off a screen and types it. Ambiguous glyphs are the
/// difference between pairing on the first try and pairing on the fourth, and
/// the entropy cost of dropping six characters from the alphabet is nil at this
/// length given the lockout.
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A pairing code with the moment it was issued.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Code {
    value: String,
    issued_at: i64,
}

/// The live pairing state: one persistent secret, one rotating code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    secret: String,

    /// Present only while a code is being displayed. `None` means the panel is
    /// closed, which is the state the app spends almost all its time in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<Code>,

    /// Human-readable note about what paired, e.g. a script version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired_label: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired_at: Option<i64>,

    #[serde(default)]
    failed_attempts: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    locked_until: Option<i64>,
}

impl Pairing {
    /// A brand-new pairing with a fresh 256-bit secret.
    pub fn generate() -> Result<Self> {
        Ok(Self {
            secret: random_hex(SECRET_BYTES)?,
            code: None,
            paired_label: None,
            paired_at: None,
            failed_attempts: 0,
            locked_until: None,
        })
    }

    /// Load from persisted JSON, or create a fresh pairing if absent or
    /// unreadable.
    ///
    /// A corrupt file yields a new secret rather than a hard failure. The cost
    /// is that the user re-pairs once; the alternative is an app that will not
    /// start because of a file it could have rewritten.
    pub fn load_or_generate(json: Option<&str>) -> Result<(Self, bool)> {
        if let Some(text) = json {
            if let Ok(p) = serde_json::from_str::<Pairing>(text) {
                if !p.secret.is_empty() {
                    // A code never survives a restart: it is display state.
                    let mut p = p;
                    p.code = None;
                    p.locked_until = None;
                    p.failed_attempts = 0;
                    return Ok((p, false));
                }
            }
        }
        Ok((Self::generate()?, true))
    }

    /// Serialize for persistence.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// The persistent install secret. Shown to the user only via
    /// [`Self::redeem`], never rendered in the UI.
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// Constant-time check of a presented bearer secret.
    pub fn verify(&self, presented: &str) -> bool {
        constant_time_eq(self.secret.as_bytes(), presented.as_bytes())
    }

    pub fn is_paired(&self) -> bool {
        self.paired_at.is_some()
    }

    /// Start displaying a code, rotating it if the current one has expired.
    ///
    /// Called by the UI while the pairing panel is open. Returns the code to
    /// display.
    pub fn show_code(&mut self, now: i64) -> Result<String> {
        let stale = match &self.code {
            Some(c) => now - c.issued_at >= CODE_TTL_SECS,
            None => true,
        };
        if stale {
            let value = random_code(CODE_CHARS)?;
            self.code = Some(Code { value, issued_at: now });
        }
        Ok(self.code.as_ref().expect("just set").value.clone())
    }

    /// The code currently on screen, if any. `None` when the panel is closed
    /// or the code has expired.
    pub fn current_code(&self, now: i64) -> Option<&str> {
        match &self.code {
            Some(c) if now - c.issued_at < CODE_TTL_SECS => Some(&c.value),
            _ => None,
        }
    }

    /// Seconds until the displayed code rotates.
    pub fn seconds_remaining(&self, now: i64) -> i64 {
        match &self.code {
            Some(c) => (CODE_TTL_SECS - (now - c.issued_at)).max(0),
            None => 0,
        }
    }

    /// Stop displaying a code. Called when the pairing panel closes, so a code
    /// is never valid while it is not on screen.
    pub fn hide_code(&mut self) {
        self.code = None;
    }

    /// Exchange a displayed code for the persistent secret.
    ///
    /// The code is *not* single-use: a user who installs the script on a second
    /// browser during the same window would otherwise have to wait for a
    /// rotation. The TTL and the lockout are what bound guessing, not the
    /// single-use property.
    pub fn redeem(&mut self, presented: &str, label: Option<String>, now: i64) -> Result<String> {
        if let Some(until) = self.locked_until {
            if now < until {
                return Err(Error::invalid(format!(
                    "too many wrong codes; try again in {} seconds",
                    until - now
                )));
            }
            self.locked_until = None;
            self.failed_attempts = 0;
        }

        let normalised = normalise_code(presented);
        let matches = match self.current_code(now) {
            Some(valid) => constant_time_eq(
                normalise_code(valid).as_bytes(),
                normalised.as_bytes(),
            ),
            None => false,
        };

        if !matches {
            self.failed_attempts += 1;
            if self.failed_attempts >= MAX_ATTEMPTS {
                self.locked_until = Some(now + LOCKOUT_SECS);
                self.failed_attempts = 0;
            }
            // One message for "wrong" and "expired" alike. Distinguishing them
            // tells a guesser whether they are close, and tells a legitimate
            // user nothing they cannot see on screen.
            return Err(Error::invalid(
                "that pairing code is not valid — check the code shown in the app",
            ));
        }

        self.failed_attempts = 0;
        self.paired_label = label;
        self.paired_at = Some(now);
        Ok(self.secret.clone())
    }

    /// Forget the secret and everything paired to it, and mint a new one.
    pub fn revoke(&mut self) -> Result<String> {
        self.secret = random_hex(SECRET_BYTES)?;
        self.code = None;
        self.paired_label = None;
        self.paired_at = None;
        self.failed_attempts = 0;
        self.locked_until = None;
        Ok(self.secret.clone())
    }
}

/// Strip everything a human might reasonably type along with the code:
/// spaces, the display separator, and case.
fn normalise_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Render a code for display as `XXXX-XXXX`.
pub fn format_code(code: &str) -> String {
    if code.len() == CODE_CHARS {
        format!("{}-{}", &code[..CODE_CHARS / 2], &code[CODE_CHARS / 2..])
    } else {
        code.to_string()
    }
}

/// Length-independent constant-time byte comparison.
///
/// The length is allowed to leak: both secrets here are fixed-length by
/// construction, so the length carries no information.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Hex-encode `n` bytes from the operating system CSPRNG.
fn random_hex(n: usize) -> Result<String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf)
        .map_err(|e| Error::invalid(format!("no system randomness available: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// A code of `len` characters drawn uniformly from [`ALPHABET`].
///
/// Rejection sampling rather than a modulo: `256 % 30 != 0`, so a plain modulo
/// would bias the first few characters of the alphabet. The bias is small, but
/// so is the fix.
fn random_code(len: usize) -> Result<String> {
    let mut out = String::with_capacity(len);
    let limit = (256 / ALPHABET.len()) * ALPHABET.len();
    let mut buf = [0u8; 32];
    while out.len() < len {
        getrandom::fill(&mut buf)
            .map_err(|e| Error::invalid(format!("no system randomness available: {e}")))?;
        for &b in buf.iter() {
            if (b as usize) < limit {
                out.push(ALPHABET[(b as usize) % ALPHABET.len()] as char);
                if out.len() == len {
                    break;
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_pairing_has_a_long_secret_and_no_code() {
        let p = Pairing::generate().unwrap();
        assert_eq!(p.secret().len(), SECRET_BYTES * 2);
        assert!(p.current_code(0).is_none(), "a code must not exist until asked for");
        assert!(!p.is_paired());
    }

    #[test]
    fn two_pairings_never_share_a_secret() {
        let a = Pairing::generate().unwrap();
        let b = Pairing::generate().unwrap();
        assert_ne!(a.secret(), b.secret());
    }

    #[test]
    fn verify_accepts_only_the_right_secret() {
        let p = Pairing::generate().unwrap();
        assert!(p.verify(p.secret()));
        assert!(!p.verify(""));
        assert!(!p.verify(&"0".repeat(SECRET_BYTES * 2)));
        // Same length, one character different.
        let mut wrong = p.secret().to_string();
        wrong.replace_range(0..1, if p.secret().starts_with('a') { "b" } else { "a" });
        assert!(!p.verify(&wrong));
    }

    #[test]
    fn a_code_is_displayed_then_rotates_on_schedule() {
        let mut p = Pairing::generate().unwrap();
        let first = p.show_code(1000).unwrap();
        // Still the same code inside the window.
        assert_eq!(p.show_code(1000 + CODE_TTL_SECS - 1).unwrap(), first);
        // A different one after it.
        let second = p.show_code(1000 + CODE_TTL_SECS).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn an_expired_code_stops_being_current_without_being_replaced() {
        let mut p = Pairing::generate().unwrap();
        p.show_code(1000).unwrap();
        assert!(p.current_code(1000 + CODE_TTL_SECS).is_none());
    }

    #[test]
    fn hiding_the_code_invalidates_it_immediately() {
        // The panel closing must not leave a live code behind.
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        p.hide_code();
        assert!(p.current_code(1000).is_none());
        assert!(p.redeem(&code, None, 1000).is_err());
    }

    #[test]
    fn redeeming_the_displayed_code_yields_the_secret() {
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        let secret = p.redeem(&code, Some("Tampermonkey".into()), 1000).unwrap();
        assert_eq!(secret, p.secret());
        assert!(p.is_paired());
        assert_eq!(p.paired_label.as_deref(), Some("Tampermonkey"));
    }

    #[test]
    fn a_redeemed_code_still_works_inside_its_window() {
        // A second browser during the same window should not need a rotation.
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        assert!(p.redeem(&code, None, 1001).is_ok());
        assert!(p.redeem(&code, None, 1002).is_ok());
    }

    #[test]
    fn the_user_may_type_the_code_without_the_dash_or_the_right_case() {
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        let typed = format_code(&code).to_lowercase().replace('-', " ");
        assert!(p.redeem(&typed, None, 1000).is_ok(), "typed {typed:?} for {code:?}");
    }

    #[test]
    fn format_code_inserts_a_single_dash_in_the_middle() {
        let f = format_code("ABCD2345");
        assert_eq!(f, "ABCD-2345");
        assert_eq!(normalise_code(&f), "ABCD2345");
    }

    #[test]
    fn a_wrong_code_locks_the_endpoint_after_repeated_attempts() {
        let mut p = Pairing::generate().unwrap();
        p.show_code(1000).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert!(p.redeem("WRONG234", None, 1000).is_err());
        }
        // Locked now, and the lockout reports itself distinctly so a legitimate
        // user understands why even the right code is refused.
        let err = p.redeem("WRONG234", None, 1000).unwrap_err().to_string();
        assert!(err.contains("too many wrong codes"), "{err}");
    }

    #[test]
    fn the_lockout_expires() {
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            let _ = p.redeem("WRONG234", None, 1000);
        }
        // Past the lockout the correct code works again — and the code has not
        // rotated, because it is still inside its own TTL.
        let secret = p.redeem(&code, None, 1000 + LOCKOUT_SECS).unwrap();
        assert_eq!(secret, p.secret());
    }

    #[test]
    fn an_expired_code_is_refused_without_saying_it_expired() {
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        let err = p.redeem(&code, None, 1000 + CODE_TTL_SECS).unwrap_err().to_string();
        assert!(!err.to_lowercase().contains("expir"), "must not distinguish: {err}");
    }

    #[test]
    fn revoking_mints_a_new_secret_and_forgets_the_pairing() {
        let mut p = Pairing::generate().unwrap();
        let before = p.secret().to_string();
        let code = p.show_code(1000).unwrap();
        p.redeem(&code, Some("x".into()), 1000).unwrap();

        let after = p.revoke().unwrap();
        assert_ne!(before, after);
        assert!(!p.is_paired());
        assert!(p.paired_label.is_none());
        assert!(!p.verify(&before), "the old secret must stop working");
        assert!(p.verify(&after));
    }

    #[test]
    fn a_secret_survives_a_round_trip_through_json() {
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        p.redeem(&code, Some("script".into()), 1000).unwrap();
        let json = p.to_json().unwrap();

        let (loaded, created) = Pairing::load_or_generate(Some(&json)).unwrap();
        assert!(!created);
        assert_eq!(loaded.secret(), p.secret());
        assert_eq!(loaded.paired_label.as_deref(), Some("script"));
    }

    #[test]
    fn a_code_never_survives_a_restart() {
        // Display state must not be resurrected from disk.
        let mut p = Pairing::generate().unwrap();
        let code = p.show_code(1000).unwrap();
        let json = p.to_json().unwrap();
        let (mut loaded, _) = Pairing::load_or_generate(Some(&json)).unwrap();
        assert!(loaded.current_code(1000).is_none());
        assert!(loaded.redeem(&code, None, 1000).is_err());
    }

    #[test]
    fn a_corrupt_state_file_yields_a_new_pairing_instead_of_failing() {
        // An unreadable file should cost one re-pair, not a broken app.
        for bad in ["", "{", "null", r#"{"secret":""}"#, "[]"] {
            let (p, created) = Pairing::load_or_generate(Some(bad)).unwrap();
            assert!(created, "input {bad:?} should have generated");
            assert_eq!(p.secret().len(), SECRET_BYTES * 2);
        }
    }

    #[test]
    fn codes_are_drawn_from_the_unambiguous_alphabet() {
        for _ in 0..50 {
            let c = random_code(CODE_CHARS).unwrap();
            assert_eq!(c.len(), CODE_CHARS);
            for ch in c.chars() {
                assert!(
                    ALPHABET.contains(&(ch as u8)),
                    "code {c:?} contains {ch:?}, which is not in the alphabet"
                );
            }
        }
    }

    #[test]
    fn codes_vary() {
        // A weak RNG would show up as repeats across a small sample.
        let mut seen = std::collections::HashSet::new();
        let mut p = Pairing::generate().unwrap();
        for i in 0..64 {
            seen.insert(p.show_code(i * CODE_TTL_SECS).unwrap());
        }
        assert_eq!(seen.len(), 64, "expected 64 distinct codes");
    }

    #[test]
    fn constant_time_eq_is_still_correct() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
