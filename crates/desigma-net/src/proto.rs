//! Handshake messages (JSON in the first frames of the bidirectional stream).
use crate::JoinError;
use serde::{Deserialize, Serialize};

/// Bump on any incompatible wire change; peers must match exactly.
/// 2: image payloads are base64, `Hello::resume`, resume tokens.
/// 3: keyed document collections (styles, variables, comments, votes, flows, guides, timelines)
/// travel as per-entry `Op::DocEntry`; `DocPatch` carries only whole-value fields; `Ack::entries`
/// and `Fix::Entries`.
pub const PROTOCOL_VERSION: u32 = 3;

/// Handshake frames are tiny; refuse anything bigger before a guest is trusted.
pub const MAX_HANDSHAKE: usize = 16 * 1024;

/// Application close codes.
pub const CLOSE_LEFT: u32 = 0;
pub const CLOSE_KICKED: u32 = 1;
pub const CLOSE_ENDED: u32 = 2;
pub const CLOSE_REFUSED: u32 = 3;
/// Only sent by [`crate::Host::sever`]; the guest treats it as a lost connection.
pub const CLOSE_LOST: u32 = 4;

#[derive(Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub password: Option<String>,
    /// Secret from an earlier visit of this session: the host may let the same guest back in
    /// without asking again. Opaque to this crate (the sync layer issues and checks it).
    #[serde(default)]
    pub resume: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub enum Reply {
    Welcome { peer: u32 },
    Refused(JoinError),
}

/// Trim, drop control characters and cap the length of a display name.
pub fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control()).take(48).collect();
    let s = s.trim();
    if s.is_empty() {
        "Guest".into()
    } else {
        s.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Ann\n"), "Ann");
        assert_eq!(clean_name("\u{7}"), "Guest");
        assert_eq!(clean_name(&"x".repeat(100)).len(), 48);
    }
}
