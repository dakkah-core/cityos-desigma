//! What a host and a guest say to each other on the reliable lane, and the UI-free host-side
//! routing that every host (the desktop app, `desigma-server`) does the same way.
//!
//! The enums are the JSON on the wire; the shapes must not change, or old and new peers stop
//! understanding each other. The routing functions are generic over [`HostWire`], the few
//! transport calls they need, so the desktop host (behind its test-friendly `HostLink`) and the
//! headless one (straight on `desigma_net::Host`) share one sequence:
//!
//! 1. `PeerJoined`: [`admit`]. The resume secret the transport saw ([`HostWire::join_token`])
//!    gives the guest its old peer back ([`HostSession::resume`]); otherwise
//!    [`HostSession::add_peer`] and [`HostWire::allow_resume`] for the new secret. A stale
//!    connection of the same peer is kicked. Then `Role`, `Images` (digests, when there are
//!    images), `Snapshot` and the `Resume` message go out, in that order. The caller then replays
//!    known presence to the newcomer.
//! 2. `Message`: `ToHost::Sync` goes to [`HostSession::receive`] and its [`Out`]s are fanned out
//!    with [`route`]; `ToHost::Fonts` is answered by [`answer_fonts`].
//! 3. `Presence`: stamp the sender and pass it to everybody else ([`relay`]).

use crate::{ClientMsg, Dest, HostSession, Out, PeerId, Role, ServerMsg};
use desigma_doc::Document;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Most fonts one request may ask for.
pub const MAX_FONTS_PER_REQUEST: usize = 64;

/// Host to guest, over the reliable lane.
#[derive(Serialize, Deserialize)]
pub enum ToGuest {
    Sync(ServerMsg),
    /// What the guest may do. Sent before the snapshot and whenever the host changes it.
    Role(Role),
    /// The digest of every embedded image, sent before the snapshot, so a guest can read the
    /// ones it has cached instead of asking for them.
    Images(Vec<ImageInfo>),
    /// A font the guest asked for. Empty `bytes`: the host cannot share it.
    Font {
        key: FontKey,
        #[serde(with = "crate::b64", default)]
        bytes: Vec<u8>,
    },
}

/// Guest to host, over the reliable lane.
#[derive(Serialize, Deserialize)]
pub enum ToHost {
    Sync(ClientMsg),
    /// Fonts the guest lacks and the document uses.
    Fonts(Vec<FontKey>),
}

/// The digest of an embedded image, by its key in the document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub key: String,
    pub digest: String,
}

/// One font face, as a guest asks for it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FontKey {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

/// SHA-256 of `bytes` as lowercase hex.
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The digest of every non-empty embedded image, by key.
pub fn image_index(doc: &Document) -> Vec<ImageInfo> {
    doc.images.iter().filter(|(_, b)| !b.is_empty()).map(|(k, b)| ImageInfo { key: k.clone(), digest: digest(b) }).collect()
}

/// Serializes a wire message; empty if it cannot be (it always can).
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    serde_json::to_vec(msg).unwrap_or_default()
}

/// The transport calls host-side routing needs. Peer numbers are the transport's own.
pub trait HostWire {
    fn send(&self, net: u32, bytes: Vec<u8>);
    fn send_presence(&self, net: u32, bytes: &[u8]);
    fn kick(&self, net: u32);
    /// The resume secret `net` showed when it connected, if any (asked once, right after the join).
    fn join_token(&self, net: u32) -> Option<String>;
    /// A guest holding this secret comes back without being asked about again.
    fn allow_resume(&self, token: &str);
}

/// A guest let into the session.
pub struct Joined {
    /// The session peer index.
    pub peer: PeerId,
    /// It came back with its old secret: same index, same id range.
    pub resumed: bool,
    /// What it may do (a returning guest keeps the role it had).
    pub role: Role,
}

/// Admits the transport peer `net` and sends it everything it needs to start; see the module docs
/// for the order. `default_role` is for newcomers. `connected` gives the transport peer that
/// currently holds a session peer: a returning guest's old connection may not have been noticed
/// as dead yet, so it is kicked (the caller replaces its entry). The host's own edits since the
/// last [`HostSession::local_changes`] must be delivered before calling this, so the snapshot is
/// not behind.
pub fn admit(
    sess: &mut HostSession,
    wire: &(impl HostWire + ?Sized),
    doc: &Document,
    net: u32,
    default_role: Role,
    connected: impl Fn(PeerId) -> Option<u32>,
) -> Joined {
    let resumed = wire.join_token(net).and_then(|t| sess.resume(&t, default_role));
    let peer = resumed.unwrap_or_else(|| sess.add_peer(default_role));
    let role = sess.role(peer).unwrap_or(default_role);
    if resumed.is_none() {
        if let Some(t) = sess.token(peer) {
            wire.allow_resume(t);
        }
    }
    if let Some(old) = connected(peer).filter(|old| *old != net) {
        wire.kick(old);
    }
    wire.send(net, encode(&ToGuest::Role(role)));
    let images = image_index(doc);
    if !images.is_empty() {
        wire.send(net, encode(&ToGuest::Images(images)));
    }
    wire.send(net, encode(&ToGuest::Sync(sess.snapshot(peer))));
    if let Some(m) = sess.resume_msg(peer) {
        wire.send(net, encode(&ToGuest::Sync(m)));
    }
    Joined { peer, resumed: resumed.is_some(), role }
}

/// Sends each [`Out`] to the guests it is for. `peers` lists session peer index and transport number.
pub fn route(wire: &(impl HostWire + ?Sized), outs: Vec<Out>, peers: impl Iterator<Item = (PeerId, u32)> + Clone) {
    for out in outs {
        let bytes = encode(&ToGuest::Sync(out.msg));
        for (cp, net) in peers.clone() {
            let hit = match out.to {
                Dest::All => true,
                Dest::AllBut(x) => cp != x,
                Dest::Peer(x) => cp == x,
            };
            if hit {
                wire.send(net, bytes.clone());
            }
        }
    }
}

/// Sends presence `bytes` of session peer `from` to every other guest.
pub fn relay(wire: &(impl HostWire + ?Sized), from: PeerId, bytes: &[u8], peers: impl Iterator<Item = (PeerId, u32)>) {
    for (cp, net) in peers {
        if cp != from {
            wire.send_presence(net, bytes);
        }
    }
}

/// Answers a guest's font request: one `Font` per key (at most [`MAX_FONTS_PER_REQUEST`]).
/// `file` gives the bytes of a font this host may share; `None` is answered with empty bytes.
pub fn answer_fonts(wire: &(impl HostWire + ?Sized), net: u32, keys: Vec<FontKey>, mut file: impl FnMut(&FontKey) -> Option<Vec<u8>>) {
    for key in keys.into_iter().take(MAX_FONTS_PER_REQUEST) {
        let bytes = file(&key).unwrap_or_default();
        wire.send(net, encode(&ToGuest::Font { key, bytes }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[test]
    fn json_shapes_are_fixed() {
        let key = FontKey { family: "Inter".into(), weight: 400, italic: false };
        let info = ImageInfo { key: "img-1".into(), digest: "ab".into() };
        assert_eq!(serde_json::to_string(&ToGuest::Role(Role::Editor)).unwrap(), r#"{"Role":"Editor"}"#);
        assert_eq!(serde_json::to_string(&ToGuest::Images(vec![info])).unwrap(), r#"{"Images":[{"key":"img-1","digest":"ab"}]}"#);
        assert_eq!(
            serde_json::to_string(&ToGuest::Font { key: key.clone(), bytes: vec![1, 2, 3] }).unwrap(),
            r#"{"Font":{"key":{"family":"Inter","weight":400,"italic":false},"bytes":"AQID"}}"#
        );
        assert_eq!(serde_json::to_string(&ToHost::Fonts(vec![key])).unwrap(), r#"{"Fonts":[{"family":"Inter","weight":400,"italic":false}]}"#);
        let sync = ServerMsg::Resume { token: "t".into() };
        let inner = serde_json::to_string(&sync).unwrap();
        assert_eq!(serde_json::to_string(&ToGuest::Sync(sync)).unwrap(), format!(r#"{{"Sync":{inner}}}"#));
        let c = ClientMsg::ImageRequest { hash: "h".into() };
        let inner = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::to_string(&ToHost::Sync(c)).unwrap(), format!(r#"{{"Sync":{inner}}}"#));
        // an empty font answer may leave `bytes` out
        let ToGuest::Font { bytes, .. } = serde_json::from_str(r#"{"Font":{"key":{"family":"A","weight":700,"italic":true}}}"#).unwrap() else { panic!() };
        assert!(bytes.is_empty());
    }

    #[test]
    fn digest_is_sha256_hex() {
        assert_eq!(digest(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[derive(Default)]
    struct Fake {
        sent: RefCell<Vec<(u32, Vec<u8>)>>,
        kicked: RefCell<Vec<u32>>,
        allowed: RefCell<Vec<String>>,
        tokens: RefCell<BTreeMap<u32, String>>,
    }
    impl HostWire for Fake {
        fn send(&self, net: u32, bytes: Vec<u8>) {
            self.sent.borrow_mut().push((net, bytes));
        }
        fn send_presence(&self, _: u32, _: &[u8]) {}
        fn kick(&self, net: u32) {
            self.kicked.borrow_mut().push(net);
        }
        fn join_token(&self, net: u32) -> Option<String> {
            self.tokens.borrow_mut().remove(&net)
        }
        fn allow_resume(&self, token: &str) {
            self.allowed.borrow_mut().push(token.to_string());
        }
    }

    fn kinds(w: &Fake) -> Vec<&'static str> {
        w.sent
            .borrow_mut()
            .drain(..)
            .map(|(_, b)| match serde_json::from_slice::<ToGuest>(&b).unwrap() {
                ToGuest::Role(_) => "role",
                ToGuest::Images(_) => "images",
                ToGuest::Font { .. } => "font",
                ToGuest::Sync(ServerMsg::Snapshot { .. }) => "snapshot",
                ToGuest::Sync(ServerMsg::Resume { .. }) => "resume",
                ToGuest::Sync(_) => "sync",
            })
            .collect()
    }

    #[test]
    fn admit_sends_in_order_and_a_returning_guest_keeps_its_peer() {
        let mut doc = Document::new("D");
        doc.images.insert("img-1".into(), vec![1, 2, 3]);
        let mut sess = HostSession::new(&mut doc);
        let w = Fake::default();
        let first = admit(&mut sess, &w, &doc, 7, Role::Editor, |_| None);
        assert!(!first.resumed);
        assert_eq!(kinds(&w), ["role", "images", "snapshot", "resume"]);
        let token = sess.token(first.peer).unwrap().to_string();
        assert_eq!(w.allowed.borrow().as_slice(), std::slice::from_ref(&token));
        // the guest comes back on another connection while the old one still looks alive
        w.tokens.borrow_mut().insert(9, token);
        let back = admit(&mut sess, &w, &doc, 9, Role::Viewer, |p| (p == first.peer).then_some(7));
        assert!(back.resumed);
        assert_eq!((back.peer, back.role), (first.peer, Role::Editor));
        assert_eq!(*w.kicked.borrow(), [7]);
        assert_eq!(w.allowed.borrow().len(), 1, "no new secret for a returning guest");
        assert_eq!(kinds(&w), ["role", "images", "snapshot", "resume"]);
        // an unknown secret is a newcomer
        w.tokens.borrow_mut().insert(11, "nope".into());
        let other = admit(&mut sess, &w, &doc, 11, Role::Viewer, |_| None);
        assert!(!other.resumed);
        assert_ne!(other.peer, first.peer);
    }

    #[test]
    fn route_honours_destinations_and_fonts_answer_empty() {
        let w = Fake::default();
        let msg = ServerMsg::Resume { token: String::new() };
        let outs = vec![Out { to: Dest::AllBut(2), msg: msg.clone() }, Out { to: Dest::Peer(2), msg: msg.clone() }, Out { to: Dest::All, msg }];
        route(&w, outs, [(1, 10), (2, 20)].into_iter());
        let nets: Vec<u32> = w.sent.borrow().iter().map(|(n, _)| *n).collect();
        assert_eq!(nets, [10, 20, 10, 20]);
        w.sent.borrow_mut().clear();
        let keys: Vec<FontKey> = (0..100).map(|i| FontKey { family: format!("F{i}"), weight: 400, italic: false }).collect();
        answer_fonts(&w, 5, keys, |k| (k.family == "F0").then(|| vec![9]));
        let sent = w.sent.borrow();
        assert_eq!(sent.len(), MAX_FONTS_PER_REQUEST);
        let ToGuest::Font { bytes, .. } = serde_json::from_slice(&sent[0].1).unwrap() else { panic!() };
        assert_eq!(bytes, [9]);
        let ToGuest::Font { bytes, .. } = serde_json::from_slice(&sent[1].1).unwrap() else { panic!() };
        assert!(bytes.is_empty());
    }
}
