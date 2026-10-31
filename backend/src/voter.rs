//! Server-issued pseudonymous voter identity for public votes.
//!
//! A public vote counts toward a request's displayed demand, so the key it is
//! deduplicated on must be one this sidecar issued, not one the caller picked.
//! The sidecar mints an opaque voter token, signs it with an HMAC key that is
//! generated once and kept in the app's own database, and hands it to the browser
//! as an HttpOnly cookie in this app's cookie namespace (the manifest lets Core
//! forward exactly that cookie). Only a token whose signature verifies is used as
//! the stored voter key; anything a caller sends in a header is ignored.
//!
//! A visitor who discards the cookie gets a new identity, so issuance is bounded
//! PER CLIENT: Core stamps a pseudonymous rate subject for each caller (the
//! manifest opts into `forward_caller_rate_subject`; a caller cannot set or
//! reset it), and one subject may be issued at most
//! [`MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR`] identities an hour and
//! [`MAX_NEW_VOTERS_PER_CLIENT_PER_DAY`] a day. A node-wide backstop
//! ([`MAX_NEW_VOTERS_PER_MINUTE`], [`MAX_NEW_VOTERS_PER_HOUR`]) also applies, and
//! is the only bound for callers Core cannot tell apart. The windows are kept in
//! the app database, so restarts and idle stops do not reset them. A vote that
//! would need an identity beyond a bound is refused with 429 before any row is
//! written. This bounds token rotation; it does not claim that one cookie is one
//! human.

use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use crate::store::{IssuanceLimits, Store};

/// The cookie name. It must stay inside Core's derived cookie namespace for
/// `@ryu/feedback-board` (`ryu_feedback-board_`) and be listed in the manifest's
/// `forward_cookie_names`, or Core strips it from the hop.
pub const VOTER_COOKIE: &str = "ryu_feedback-board_voter";
/// One year: the identity should outlive a browsing session so repeat visits do
/// not consume the issuance budget.
pub const VOTER_COOKIE_MAX_AGE_SECS: u64 = 365 * 24 * 60 * 60;
/// Node-wide backstop, for every caller together.
pub const MAX_NEW_VOTERS_PER_MINUTE: i64 = 60;
pub const MAX_NEW_VOTERS_PER_HOUR: i64 = 600;
/// Per client (Core's rate subject).
pub const MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR: i64 = 5;
pub const MAX_NEW_VOTERS_PER_CLIENT_PER_DAY: i64 = 20;
/// The header Core stamps the caller's rate subject in. Core strips any
/// caller-supplied copy before the hop.
pub const RATE_SUBJECT_HEADER: &str = "x-ryu-caller-rate-subject";

const LIMITS: IssuanceLimits = IssuanceLimits {
    per_minute: MAX_NEW_VOTERS_PER_MINUTE,
    per_hour: MAX_NEW_VOTERS_PER_HOUR,
    per_client_per_hour: MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR,
    per_client_per_day: MAX_NEW_VOTERS_PER_CLIENT_PER_DAY,
};

const SECRET_NAME: &str = "voter_token_hmac_v1";
const TOKEN_PREFIX: &str = "v1";
const MAC_DOMAIN: &[u8] = b"ryu-feedback-board-voter-v1:";

type HmacSha256 = Hmac<Sha256>;

/// A freshly issued identity: the stored voter key plus the cookie value.
pub struct IssuedVoter {
    pub voter_key: String,
    pub token: String,
}

pub struct VoterTokens {
    key: Vec<u8>,
}

/// Core's rate subject (`user:<24 hex>` or `peer:<24 hex>`), or `None` when the
/// header is absent, `unknown`, or malformed. `None` callers share the node-wide
/// backstop only.
pub fn rate_subject(value: Option<&str>) -> Option<&str> {
    let value = value?;
    let (kind, hash) = value.split_once(':')?;
    let well_formed = matches!(kind, "user" | "peer")
        && hash.len() == 24
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    well_formed.then_some(value)
}

impl VoterTokens {
    /// Load (or create, once) the signing key from the app's database.
    pub fn load(store: &Store) -> Result<Self> {
        let key = store.secret_or_insert(SECRET_NAME, || {
            let mut bytes = Vec::with_capacity(32);
            bytes.extend_from_slice(Uuid::new_v4().as_bytes());
            bytes.extend_from_slice(Uuid::new_v4().as_bytes());
            bytes
        })?;
        Ok(Self { key })
    }

    fn mac(&self, nonce: &str) -> HmacSha256 {
        let mut mac =
            HmacSha256::new_from_slice(&self.key).expect("HMAC accepts a key of any length");
        mac.update(MAC_DOMAIN);
        mac.update(nonce.as_bytes());
        mac
    }

    /// The stored voter key for a cookie value, or `None` when the value is not a
    /// token this sidecar signed.
    pub fn verify(&self, token: &str) -> Option<String> {
        let mut parts = token.split('.');
        let (Some(prefix), Some(nonce), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        if prefix != TOKEN_PREFIX
            || nonce.len() != 32
            || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        let signature = hex::decode(signature).ok()?;
        self.mac(nonce).verify_slice(&signature).ok()?;
        Some(voter_key(nonce))
    }

    /// Mint a new identity for a caller with rate subject `subject`, or `None`
    /// when that client's (or the node-wide) issuance bound is spent.
    pub fn issue(&self, store: &Store, subject: Option<&str>) -> Result<Option<IssuedVoter>> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default();
        self.issue_at(store, subject, now_ms)
    }

    fn issue_at(
        &self,
        store: &Store,
        subject: Option<&str>,
        now_ms: i64,
    ) -> Result<Option<IssuedVoter>> {
        if !store.claim_voter_issuance(subject, now_ms, &LIMITS)? {
            return Ok(None);
        }
        let nonce = Uuid::new_v4().simple().to_string();
        let signature = hex::encode(self.mac(&nonce).finalize().into_bytes());
        Ok(Some(IssuedVoter {
            voter_key: voter_key(&nonce),
            token: format!("{TOKEN_PREFIX}.{nonce}.{signature}"),
        }))
    }
}

/// Namespaced so a verified identity can never equal a legacy caller-chosen key.
fn voter_key(nonce: &str) -> String {
    format!("signed:{nonce}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE_MS: i64 = 60 * 1000;
    const HOUR_MS: i64 = 60 * MINUTE_MS;
    const PEER_A: &str = "peer:aaaaaaaaaaaaaaaaaaaaaaaa";
    const PEER_B: &str = "peer:bbbbbbbbbbbbbbbbbbbbbbbb";

    fn setup() -> (Store, VoterTokens) {
        let store = Store::open_memory().unwrap();
        let voters = VoterTokens::load(&store).unwrap();
        (store, voters)
    }

    #[test]
    fn an_issued_token_verifies_to_its_voter_key() {
        let (store, voters) = setup();
        let issued = voters.issue(&store, None).unwrap().unwrap();
        assert_eq!(voters.verify(&issued.token), Some(issued.voter_key));
    }

    #[test]
    fn unsigned_tampered_or_foreign_tokens_are_rejected() {
        let (store, voters) = setup();
        let issued = voters.issue(&store, None).unwrap().unwrap();
        let (head, _) = issued.token.rsplit_once('.').unwrap();
        assert_eq!(voters.verify(&format!("{head}.{}", "0".repeat(64))), None);
        assert_eq!(voters.verify("stable"), None);
        assert_eq!(voters.verify("rotating-1"), None);
        assert_eq!(voters.verify(""), None);
        let mut other_nonce = issued.token.clone();
        other_nonce.replace_range(3..4, if &other_nonce[3..4] == "a" { "b" } else { "a" });
        assert_eq!(voters.verify(&other_nonce), None);
        // A token signed under a different node's key does not verify here.
        assert_eq!(setup().1.verify(&issued.token), None);
    }

    #[test]
    fn the_signing_key_survives_a_restart() {
        let store = Store::open_memory().unwrap();
        let first = VoterTokens::load(&store).unwrap();
        let issued = first.issue(&store, None).unwrap().unwrap();
        let reopened = VoterTokens::load(&store).unwrap();
        assert_eq!(reopened.verify(&issued.token), Some(issued.voter_key));
    }

    #[test]
    fn only_cores_subject_shapes_are_per_client() {
        assert_eq!(rate_subject(Some(PEER_A)), Some(PEER_A));
        assert_eq!(
            rate_subject(Some("user:0123456789abcdef01234567")),
            Some("user:0123456789abcdef01234567")
        );
        for other in [
            "unknown",
            "peer:short",
            "peer:AAAAAAAAAAAAAAAAAAAAAAAA",
            "ip:aaaaaaaaaaaaaaaaaaaaaaaa",
            "",
        ] {
            assert_eq!(rate_subject(Some(other)), None, "{other}");
        }
        assert_eq!(rate_subject(None), None);
    }

    /// One client is held to its own small bound, without spending another
    /// client's: peer A is refused while peer B still gets an identity.
    #[test]
    fn issuance_is_bounded_per_client_per_hour_and_per_day() {
        let (store, voters) = setup();
        let start = 1_700_000_000_000;
        for _ in 0..MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR {
            assert!(voters
                .issue_at(&store, Some(PEER_A), start)
                .unwrap()
                .is_some());
        }
        assert!(voters
            .issue_at(&store, Some(PEER_A), start)
            .unwrap()
            .is_none());
        assert!(voters
            .issue_at(&store, Some(PEER_B), start)
            .unwrap()
            .is_some());

        // The hour frees up, the day keeps counting.
        let mut at = start;
        let mut minted = MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR;
        while minted < MAX_NEW_VOTERS_PER_CLIENT_PER_DAY {
            at += HOUR_MS;
            for _ in 0..MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR
                .min(MAX_NEW_VOTERS_PER_CLIENT_PER_DAY - minted)
            {
                assert!(voters.issue_at(&store, Some(PEER_A), at).unwrap().is_some());
                minted += 1;
            }
        }
        assert!(voters
            .issue_at(&store, Some(PEER_A), at + HOUR_MS)
            .unwrap()
            .is_none());
        assert!(voters
            .issue_at(&store, Some(PEER_A), start + 24 * HOUR_MS + 1)
            .unwrap()
            .is_some());
    }

    #[test]
    fn the_node_wide_backstop_bounds_callers_core_cannot_tell_apart() {
        let (store, voters) = setup();
        let start = 1_700_000_000_000;
        for _ in 0..MAX_NEW_VOTERS_PER_MINUTE {
            assert!(voters.issue_at(&store, None, start).unwrap().is_some());
        }
        assert!(voters.issue_at(&store, None, start).unwrap().is_none());
        assert!(voters
            .issue_at(&store, None, start + MINUTE_MS)
            .unwrap()
            .is_some());
    }

    /// The windows are in the database: a new `VoterTokens` (a sidecar restart
    /// or idle stop) over the same store keeps the bound.
    #[test]
    fn the_bound_survives_a_reload() {
        let (store, voters) = setup();
        let now = 1_700_000_000_000;
        for _ in 0..MAX_NEW_VOTERS_PER_CLIENT_PER_HOUR {
            assert!(voters
                .issue_at(&store, Some(PEER_A), now)
                .unwrap()
                .is_some());
        }
        drop(voters);
        let reloaded = VoterTokens::load(&store).unwrap();
        assert!(reloaded
            .issue_at(&store, Some(PEER_A), now + 1)
            .unwrap()
            .is_none());
    }
}
