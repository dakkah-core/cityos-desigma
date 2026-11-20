//! Presence: who is here and what they are doing. Sent as small unreliable datagrams, never part
//! of the document: name, page, cursor, selection and viewport, last state wins.

use egui::Color32;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Datagrams must stay well under the ~1.2 KB the transport allows.
const MAX_BYTES: usize = 1000;
/// Longest gap between sends, so a quiet person is not mistaken for a gone one.
pub const KEEPALIVE: f64 = 2.0;
/// Shortest gap between sends (about 20 Hz).
pub const MIN_GAP: f64 = 0.05;
/// A person not heard from for this long is dropped (backup for a missed "gone").
pub const EXPIRE: f64 = 12.0;

/// One person's state. `peer` is filled in by the host when it relays a guest's message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub peer: u32,
    pub name: String,
    /// Page node id.
    pub page: u64,
    /// Pointer in page coordinates, None when it is off the canvas.
    pub cursor: Option<[f32; 2]>,
    pub sel: Vec<u64>,
    /// Viewport: centre in page coordinates, then zoom.
    pub view: [f32; 3],
    /// Sent once by the host when someone leaves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

impl Presence {
    pub fn gone(peer: u32) -> Self {
        Self { peer, name: String::new(), page: 0, cursor: None, sel: vec![], view: [0.0, 0.0, 1.0], gone: true }
    }

    /// JSON bytes under the datagram limit: the selection is cut until it fits.
    pub fn encode(&self) -> Vec<u8> {
        let mut p = self.clone();
        p.name = p.name.chars().take(48).collect();
        loop {
            let bytes = serde_json::to_vec(&p).unwrap_or_default();
            if bytes.len() <= MAX_BYTES || p.sel.is_empty() {
                return bytes;
            }
            p.sel.truncate(p.sel.len() / 2);
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut p: Presence = serde_json::from_slice(bytes).ok()?;
        p.sel.truncate(256);
        p.name = p.name.chars().filter(|c| !c.is_control()).take(48).collect();
        let ok = |v: f32| v.is_finite();
        (p.view.iter().all(|v| ok(*v)) && p.cursor.is_none_or(|c| ok(c[0]) && ok(c[1]))).then_some(p)
    }

    /// Same state, ignoring sub-pixel jitter.
    fn same_as(&self, o: &Presence) -> bool {
        let near = |a: f32, b: f32| (a - b).abs() < 0.05;
        self.name == o.name
            && self.page == o.page
            && self.sel == o.sel
            && near(self.view[0], o.view[0])
            && near(self.view[1], o.view[1])
            && near(self.view[2], o.view[2])
            && match (self.cursor, o.cursor) {
                (None, None) => true,
                (Some(a), Some(b)) => near(a[0], b[0]) && near(a[1], b[1]),
                _ => false,
            }
    }
}

/// Decides when the local presence goes out: on change at most 20 times a second, and at least
/// every [`KEEPALIVE`] seconds.
#[derive(Default)]
pub struct Sender {
    last: Option<Presence>,
    at: f64,
}

impl Sender {
    /// The bytes to send now, if any. `pending` is true while a change waits for the throttle,
    /// so the caller can schedule another frame.
    pub fn poll(&mut self, now: f64, cur: &Presence) -> (Option<Vec<u8>>, bool) {
        let changed = self.last.as_ref().is_none_or(|l| !l.same_as(cur));
        let since = now - self.at;
        if (changed && since >= MIN_GAP) || since >= KEEPALIVE || (self.last.is_none()) {
            self.last = Some(cur.clone());
            self.at = now;
            return (Some(cur.encode()), false);
        }
        (None, changed)
    }

    /// Forces the next [`Self::poll`] to send (a new guest needs the current state).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// Someone else in the session.
#[derive(Clone, Debug)]
pub struct Remote {
    pub p: Presence,
    /// Cursor as drawn: eases toward `p.cursor`.
    pub shown: Option<[f32; 2]>,
    pub seen: f64,
}

/// Colours by peer index (host 0), far enough apart to tell at a glance.
pub const PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x0C, 0x8C, 0xE9),
    Color32::from_rgb(0xF2, 0x48, 0x22),
    Color32::from_rgb(0x14, 0xAE, 0x5C),
    Color32::from_rgb(0x97, 0x47, 0xFF),
    Color32::from_rgb(0xFF, 0xA6, 0x29),
    Color32::from_rgb(0xEE, 0x46, 0xBC),
    Color32::from_rgb(0x00, 0xA9, 0xB5),
    Color32::from_rgb(0x8A, 0x9A, 0x1C),
];

pub fn color_of(peer: u32) -> Color32 {
    PALETTE[peer as usize % PALETTE.len()]
}

/// First letter of a name, upper-cased, for avatars.
pub fn initial(name: &str) -> String {
    name.trim().chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

/// Updates `remotes` with a received message (or removes the person on `gone`).
pub fn apply(remotes: &mut BTreeMap<u32, Remote>, p: Presence, now: f64) {
    if p.gone {
        remotes.remove(&p.peer);
        return;
    }
    let peer = p.peer;
    match remotes.get_mut(&peer) {
        Some(r) => {
            if r.shown.is_none() || p.page != r.p.page {
                r.shown = p.cursor;
            }
            r.p = p;
            r.seen = now;
        }
        None => {
            let shown = p.cursor;
            remotes.insert(peer, Remote { p, shown, seen: now });
        }
    }
}

/// Eases cursors toward their targets and drops people not heard from. Returns the peers that
/// expired and whether anything is still moving (the caller keeps repainting).
pub fn tick(remotes: &mut BTreeMap<u32, Remote>, now: f64, dt: f64) -> (Vec<u32>, bool) {
    let k = (1.0 - (-dt * 22.0).exp()) as f32;
    let mut moving = false;
    for r in remotes.values_mut() {
        match (r.p.cursor, r.shown.as_mut()) {
            (Some(t), Some(s)) => {
                let (dx, dy) = (t[0] - s[0], t[1] - s[1]);
                if dx.abs() + dy.abs() < 0.05 {
                    *s = t;
                } else {
                    s[0] += dx * k;
                    s[1] += dy * k;
                    moving = true;
                }
            }
            (Some(t), None) => r.shown = Some(t),
            (None, _) => r.shown = None,
        }
    }
    let gone: Vec<u32> = remotes.iter().filter(|(_, r)| now - r.seen > EXPIRE).map(|(p, _)| *p).collect();
    for p in &gone {
        remotes.remove(p);
    }
    (gone, moving)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Presence {
        Presence { peer: 1, name: "Ann".into(), page: 7, cursor: Some([10.0, 20.0]), sel: vec![1, 2, 3], view: [0.0, 0.0, 1.0], gone: false }
    }

    #[test]
    fn roundtrip_and_size_cap() {
        let mut big = p();
        big.sel = (0..5000u64).map(|i| (3 << 40) + i).collect();
        let bytes = big.encode();
        assert!(bytes.len() <= MAX_BYTES, "{}", bytes.len());
        let back = Presence::decode(&bytes).unwrap();
        assert!(!back.sel.is_empty() && back.sel.len() < 5000);
        assert_eq!(Presence::decode(&p().encode()).unwrap(), p());
    }

    #[test]
    fn sender_throttles_and_keeps_alive() {
        let mut s = Sender::default();
        let mut cur = p();
        assert!(s.poll(0.0, &cur).0.is_some());
        cur.cursor = Some([30.0, 20.0]);
        let (b, pending) = s.poll(0.01, &cur);
        assert!(b.is_none() && pending);
        assert!(s.poll(0.06, &cur).0.is_some());
        assert!(s.poll(0.5, &cur).0.is_none());
        assert!(s.poll(2.2, &cur).0.is_some());
    }

    #[test]
    fn cursors_ease_and_people_expire() {
        let mut m = BTreeMap::new();
        apply(&mut m, p(), 0.0);
        let mut q = p();
        q.cursor = Some([110.0, 20.0]);
        apply(&mut m, q, 0.1);
        let (_, moving) = tick(&mut m, 0.1, 0.016);
        let x = m[&1].shown.unwrap()[0];
        assert!(moving && x > 10.0 && x < 110.0);
        let (gone, _) = tick(&mut m, 100.0, 0.016);
        assert_eq!(gone, vec![1]);
        apply(&mut m, p(), 1.0);
        apply(&mut m, Presence::gone(1), 1.0);
        assert!(m.is_empty());
    }

    #[test]
    fn garbage_is_ignored() {
        assert!(Presence::decode(b"nope").is_none());
        assert!(Presence::decode(br#"{"peer":1,"name":"x","page":1,"cursor":null,"sel":[],"view":[0,0,1]}"#).is_some());
    }
}
