//! Bytes that travel next to the document: images (read from the local cache when the guest has
//! them) and fonts (asked from the host when the guest cannot load them itself).

use super::assets::{self, digest, font_key, FontKey};
use super::{encode, GuestState, HostState, ToHost};
use crate::app::App;
use desigma_collab::wire;
use desigma_collab::{ClientMsg, ServerMsg};
use desigma_fonts::FontRequest;
use std::collections::{BTreeSet, VecDeque};

/// What a guest has asked the host for, by font.
#[derive(Default)]
pub struct FontAsk {
    /// Everything asked this session; nothing is asked twice.
    asked: BTreeSet<FontKey>,
    /// Asked, no answer yet.
    waiting: BTreeSet<FontKey>,
    /// The host had no usable file for these.
    missing: Vec<FontKey>,
    /// Fonts the host sent and the guest loaded.
    pub loaded: Vec<FontKey>,
}

/// Checks image bytes from the host against the digest it announced, and keeps them in the cache.
/// `None` drops the message (the bytes are not what the host said they were).
pub fn vet(app: &mut App, g: &mut GuestState, msg: ServerMsg) -> Option<ServerMsg> {
    if let ServerMsg::ImageData { hash, bytes } = &msg {
        let d = digest(bytes);
        if g.digests.get(hash).is_some_and(|want| *want != d) {
            return None;
        }
        app.collab.images.put(&d, bytes);
        g.digests.insert(hash.clone(), d);
    }
    Some(msg)
}

/// Sends what the guest session wants to say to the host. An image it asks for that the cache
/// holds is answered from there (as if the host had sent it) instead.
pub fn send_out(app: &mut App, g: &mut GuestState, outs: Vec<ClientMsg>, local: &mut VecDeque<ServerMsg>) {
    for m in outs {
        if let ClientMsg::ImageRequest { hash } = &m {
            let cached = g.digests.get(hash).and_then(|d| app.collab.images.get(d));
            if let Some(bytes) = cached {
                local.push_back(ServerMsg::ImageData { hash: hash.clone(), bytes });
                continue;
            }
        }
        g.link.send(encode(&ToHost::Sync(m)));
    }
}

/// Host: sends the files of the fonts a guest asked for, if the document uses them and this
/// machine has them. Empty bytes mean "cannot share".
pub fn answer_fonts(app: &App, h: &HostState, peer: u32, keys: Vec<FontKey>) {
    let Some(p) = h.peers.get(&peer) else { return };
    let used = desigma_fonts::requests_for(&app.doc);
    wire::answer_fonts(&*h.link, p.net, keys, |key| {
        let wanted = used.contains(&FontRequest::new(key.family.clone(), key.weight, key.italic));
        if wanted {
            app.collab.font_io.0.file(key).filter(|b| b.len() <= assets::MAX_FONT)
        } else {
            None
        }
    });
}

/// Guest: a font the host sent (or could not). Returns whether the document must be redrawn.
pub fn font_arrived(app: &mut App, g: &mut GuestState, key: FontKey, bytes: Vec<u8>) -> bool {
    if !g.fonts.waiting.remove(&key) {
        return false;
    }
    let usable = !bytes.is_empty() && bytes.len() <= assets::MAX_FONT;
    if usable {
        app.collab.font_io.0.install(bytes);
        g.fonts.loaded.push(key);
    } else {
        g.fonts.missing.push(key);
    }
    if g.fonts.waiting.is_empty() && !g.fonts.missing.is_empty() {
        let names =
            std::mem::take(&mut g.fonts.missing).iter().map(|k| format!("{} {}: the host cannot share this font", k.family, k.weight)).collect::<Vec<_>>();
        app.toast_fonts_unavailable(&names);
    }
    usable
}

impl App {
    /// The background font load could not resolve `failed`. A guest asks the host for the ones
    /// the document uses; what is left (and everything, outside a session) is returned.
    pub fn ask_host_for_fonts(&mut self, failed: Vec<FontRequest>) -> Vec<FontRequest> {
        let Some(mut g) = self.collab.guest.take() else { return failed };
        if g.ended.is_some() {
            self.collab.guest = Some(g);
            return failed;
        }
        let used = desigma_fonts::requests_for(&self.doc);
        let mut rest = Vec::new();
        let mut keys = Vec::new();
        for r in failed {
            let key = font_key(&r);
            if !used.contains(&r) {
                rest.push(r);
            } else if g.live() && g.fonts.asked.insert(key.clone()) {
                g.fonts.waiting.insert(key.clone());
                keys.push(key);
            }
        }
        if !keys.is_empty() {
            g.link.send(encode(&ToHost::Fonts(keys)));
        }
        self.collab.guest = Some(g);
        rest
    }
}
