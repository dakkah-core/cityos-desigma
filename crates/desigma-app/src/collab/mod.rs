//! Live collaboration in the editor: Share, Join, remote edits, presence and Follow.
//!
//! The sync rules live in `desigma-collab` (pure, no network) and the transport in `desigma-net`.
//! This module is the glue: it owns the session on [`App`], moves messages between the two once per
//! frame ([`pump`]) and applies remote changes to `App::doc`. See `docs/collaboration.md`.
//!
//! One frame, in order: drain the transport; give guest messages to the sync core (remote ops
//! change the document in place, so the canvas is marked dirty and the undo history rebased);
//! number our own edits when the document changed (at most 20 times a second, always flushed
//! once things go quiet); send presence; ease the other people's cursors; move the view when
//! following someone.

pub mod assets;
pub mod draw;
mod exchange;
pub mod link;
pub mod presence;
mod reconnect;
#[cfg(test)]
pub(crate) mod tests;
pub mod ui;

use crate::app::App;
use crate::home::dialog::{Dialog, Intent};
use assets::{FontIoBox, ImageCache};
use desigma_collab::wire::{self, encode, ImageInfo};
pub use desigma_collab::wire::{ToGuest, ToHost};
#[cfg(test)]
use desigma_collab::ClientMsg;
use desigma_collab::{rebase_history, GuestSession, HostSession, Out, Role, ServerMsg};
use desigma_doc::NodeId;
use link::{Closed, GuestEv, GuestLink, HostEv, HostLink, Redial};
use presence::{Presence, Remote};
use reconnect::Reconnect;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// What to do once the user agrees to leave the session.
type AfterLeave = Box<dyn FnOnce(&mut App)>;

/// Local edits are numbered at most this often (20 Hz).
const FLUSH_GAP: f64 = 0.05;

/// A guest waiting for the host's Accept / Reject.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    pub name: String,
}

pub struct Peer {
    /// The transport's number for this guest.
    pub net: u32,
    pub name: String,
    pub role: Role,
}

pub struct HostState {
    pub link: Box<dyn HostLink>,
    pub sess: HostSession,
    pub url: Option<String>,
    pub password: bool,
    pub approval: bool,
    /// Role of guests who join from now on.
    pub role: Role,
    /// Guests by session peer index.
    pub peers: BTreeMap<u32, Peer>,
}

pub struct GuestState {
    pub link: Box<dyn GuestLink>,
    pub sess: GuestSession,
    pub role: Role,
    /// Set when the connection ended for good; the document stays as a local copy.
    pub ended: Option<Closed>,
    pub banner_hidden: bool,
    /// Dials the host again after a lost connection.
    pub redial: Option<Redial>,
    /// Set while the connection is down and we are dialling again; the document stays editable.
    pub reconnect: Option<Reconnect>,
    /// The host's digest of each image key.
    pub digests: HashMap<String, String>,
    pub fonts: exchange::FontAsk,
}

impl GuestState {
    /// Connected to the host right now.
    pub fn live(&self) -> bool {
        self.ended.is_none() && self.reconnect.is_none()
    }
}

/// Dialled the host, snapshot not here yet.
pub struct Joining {
    pub link: Box<dyn GuestLink>,
    pub role: Role,
    pub redial: Option<Redial>,
}

#[derive(Default)]
struct Pacing {
    rev: u64,
    at: f64,
    dirty: bool,
}

#[derive(Default)]
pub struct CollabState {
    pub host: Option<HostState>,
    pub guest: Option<GuestState>,
    pub joining: Option<Joining>,
    pub share: ui::ShareUi,
    pub join: Option<ui::JoinUi>,
    /// The other people, by session peer index.
    pub remotes: BTreeMap<u32, Remote>,
    /// Whose viewport the view tracks.
    pub following: Option<u32>,
    last_follow: Option<(f32, f32, f32, NodeId)>,
    pub requests: Vec<Request>,
    pacing: Pacing,
    pres: presence::Sender,
    last_now: f64,
    /// A `desigma://join/...` link from the command line, to open the Join dialog on.
    pub pending_link: Option<String>,
    /// Image bytes received before, by content digest.
    pub images: ImageCache,
    /// Where fonts are read from (host) and loaded to (guest).
    pub font_io: FontIoBox,
    /// What to do once the user agrees to leave the session ("Stop sharing?").
    leave: Option<AfterLeave>,
}

impl CollabState {
    pub fn hosting(&self) -> bool {
        self.host.is_some()
    }
    pub fn is_guest(&self) -> bool {
        self.guest.is_some()
    }
    /// Sharing, joined, or joining.
    pub fn active(&self) -> bool {
        self.host.is_some() || self.guest.is_some() || self.joining.is_some()
    }
    /// Connected as a guest who may not edit.
    pub fn view_only(&self) -> bool {
        self.guest.as_ref().is_some_and(|g| g.role == Role::Viewer && g.ended.is_none())
    }
    /// Our peer index: 0 for the host.
    pub fn self_peer(&self) -> Option<u32> {
        match (&self.host, &self.guest) {
            (Some(_), _) => Some(0),
            (_, Some(g)) => Some(g.sess.peer()),
            _ => None,
        }
    }
    /// A live connection (not an ended guest copy).
    pub fn connected(&self) -> bool {
        self.host.is_some() || self.guest.as_ref().is_some_and(|g| g.live())
    }
    /// The question to ask before leaving the file, if there is a live session to lose.
    fn leave_dialog(&self) -> Option<Dialog> {
        let (title, body, ok) = if let Some(h) = &self.host {
            let body = match h.peers.len() {
                0 => "Nobody is in the file now. The link stops working.".to_string(),
                1 => "1 person will be disconnected.".to_string(),
                n => format!("{n} people will be disconnected."),
            };
            ("Stop sharing?", body, "Stop sharing")
        } else if self.guest.as_ref().is_some_and(|g| g.ended.is_none()) {
            ("Leave the session?", "You will be disconnected. Save a copy first if you want to keep this file.".to_string(), "Leave")
        } else {
            return None;
        };
        let mut d = Dialog::confirm(Intent::LeaveSession, title, &[&body], ok);
        d.danger = self.host.is_some();
        Some(d)
    }
}

/// Runs the confirmed "Leave the session?" dialog.
pub fn finish_dialog(app: &mut App, d: &Dialog) -> bool {
    if d.intent != Intent::LeaveSession {
        return false;
    }
    app.end_collab();
    if let Some(then) = app.collab.leave.take() {
        then(app);
    }
    true
}

/// What the window knows that the session needs this frame.
#[derive(Default)]
pub struct Local {
    /// Pointer position in screen coordinates, when it is inside the window.
    pub pointer: Option<egui::Pos2>,
    /// Escape was pressed (stops following).
    pub esc: bool,
}

impl App {
    /// Starts sharing the open document. `password` and `approval` are fixed for the session.
    pub fn start_sharing(&mut self, ctx: &egui::Context, password: Option<String>, approval: bool, role: Role) {
        let c = ctx.clone();
        let wake: Box<dyn Fn() + Send + Sync> = Box::new(move || c.request_repaint());
        match link::NetHost::start(password.clone().filter(|p| !p.is_empty()), approval, wake) {
            Ok(h) => self.start_sharing_with(Box::new(h), password.is_some_and(|p| !p.is_empty()), approval, role),
            Err(e) => self.show_toast(format!("Cannot start sharing: {e}"), true),
        }
    }

    /// [`Self::start_sharing`] over any transport (tests use channels).
    pub fn start_sharing_with(&mut self, link: Box<dyn HostLink>, password: bool, approval: bool, role: Role) {
        if self.collab.active() {
            return;
        }
        if self.history.viewing.is_some() {
            self.show_toast("Go back to the current version before sharing", true);
            return;
        }
        let sess = HostSession::new(&mut self.doc);
        self.collab.host = Some(HostState { link, sess, url: None, password, approval, role, peers: BTreeMap::new() });
        self.collab.pacing = Pacing { rev: self.canvas.tiles.rev(), ..Default::default() };
        self.collab.pres.reset();
        self.collab.remotes.clear();
    }

    pub fn stop_sharing(&mut self) {
        if self.collab.host.take().is_some() {
            self.collab.remotes.clear();
            self.collab.following = None;
            self.collab.requests.clear();
            self.show_toast("Stopped sharing", false);
        }
    }

    /// Leaves or ends whatever session there is. The document stays.
    pub fn end_collab(&mut self) {
        self.collab.host = None;
        if let Some(g) = self.collab.guest.take() {
            g.link.leave();
        }
        if let Some(j) = self.collab.joining.take() {
            j.link.leave();
        }
        self.collab.remotes.clear();
        self.collab.following = None;
        self.collab.last_follow = None;
        self.collab.requests.clear();
    }

    /// Dials the host from a pasted link. Errors come back as text for the Join dialog.
    pub fn join_session(&mut self, ctx: &egui::Context, text: &str, name: &str, password: &str) -> Result<(), String> {
        let ticket =
            desigma_net::Ticket::parse(text).map_err(|_| "That does not look like a desigma session link. It starts with desigma://join/".to_string())?;
        if ticket.password_required && password.is_empty() {
            return Err("This session needs a password. Ask the person who shared it.".into());
        }
        let wake = |c: egui::Context| -> Box<dyn Fn() + Send + Sync> { Box::new(move || c.request_repaint()) };
        let pw = (!password.is_empty()).then_some(password);
        let g = link::NetGuest::join(&ticket, name, pw, wake(ctx.clone())).map_err(|e| format!("Cannot start joining: {e}"))?;
        let (ctx, name, password) = (ctx.clone(), name.to_string(), password.to_string());
        let redial: Redial = Box::new(move |token| {
            let pw = (!password.is_empty()).then_some(password.as_str());
            link::NetGuest::join_resume(&ticket, &name, pw, wake(ctx.clone()), token.map(str::to_string)).map(|g| Box::new(g) as Box<dyn GuestLink>)
        });
        self.join_with_redial(Box::new(g), Some(redial));
        Ok(())
    }

    /// [`Self::join_session`] over any transport.
    #[cfg(test)]
    pub fn join_with(&mut self, link: Box<dyn GuestLink>) {
        self.join_with_redial(link, None);
    }

    /// [`Self::join_with`] that can dial again when the connection drops.
    pub fn join_with_redial(&mut self, link: Box<dyn GuestLink>, redial: Option<Redial>) {
        self.end_collab();
        self.collab.joining = Some(Joining { link, role: Role::Editor, redial });
    }

    /// Whether it is fine to leave the open file now. While sharing or joined it asks first
    /// ("Stop sharing?" / "Leave the session?") and returns `false`; `then` runs once the user
    /// agrees, with the session already ended. Use it before anything that replaces the document.
    pub fn leave_ok(&mut self, then: impl FnOnce(&mut App) + 'static) -> bool {
        let Some(dialog) = self.collab.leave_dialog() else { return true };
        self.collab.leave = Some(Box::new(then));
        self.dialog = Some(dialog);
        false
    }

    /// Aborts a join that is still connecting.
    pub fn cancel_join(&mut self) {
        if let Some(j) = self.collab.joining.take() {
            j.link.leave();
        }
    }

    pub fn leave_session(&mut self) {
        if self.collab.guest.is_some() {
            self.end_collab();
            self.show_toast("Left the session. The file stays open as your own copy", false);
        }
    }

    pub fn kick_peer(&mut self, peer: u32) {
        let Some(h) = self.collab.host.as_mut() else { return };
        if let Some(p) = h.peers.remove(&peer) {
            h.link.kick(p.net);
            if let Some(t) = h.sess.token(peer) {
                h.link.revoke_resume(t);
            }
            h.sess.forget_peer(peer);
            self.collab.remotes.remove(&peer);
            if self.collab.following == Some(peer) {
                self.collab.following = None;
            }
            self.broadcast_gone(peer);
        }
    }

    pub fn set_peer_role(&mut self, peer: u32, role: Role) {
        let Some(h) = self.collab.host.as_mut() else { return };
        if let Some(p) = h.peers.get_mut(&peer) {
            p.role = role;
            h.sess.set_role(peer, role);
            h.link.send(p.net, encode(&ToGuest::Role(role)));
        }
    }

    /// Guests who join from now on, and everyone already in, get this role.
    pub fn set_all_roles(&mut self, role: Role) {
        let peers: Vec<u32> = self.collab.host.as_ref().map(|h| h.peers.keys().copied().collect()).unwrap_or_default();
        if let Some(h) = self.collab.host.as_mut() {
            h.role = role;
        }
        for p in peers {
            self.set_peer_role(p, role);
        }
    }

    pub fn respond_request(&mut self, id: u64, accept: bool) {
        if let Some(h) = &self.collab.host {
            h.link.respond(id, accept);
        }
        self.collab.requests.retain(|r| r.id != id);
    }

    pub fn toggle_follow(&mut self, peer: u32) {
        self.collab.last_follow = None;
        self.collab.following = if self.collab.following == Some(peer) { None } else { Some(peer) };
    }

    fn broadcast_gone(&mut self, peer: u32) {
        if let Some(h) = &self.collab.host {
            let bytes = Presence::gone(peer).encode();
            for p in h.peers.values() {
                h.link.send_presence(p.net, &bytes);
            }
        }
    }

    /// Our own name as other people see it.
    pub fn collab_name(&mut self) -> String {
        self.author()
    }

    /// The document may not be edited here: an old version is on screen, or this guest may only view.
    pub fn read_only(&self) -> bool {
        self.history.viewing.is_some() || self.collab.view_only()
    }

    /// Applies a change made to the document by someone else.
    fn remote_changed(&mut self) {
        self.changed();
        self.sanitize();
        self.fonts.load_for(&self.doc);
    }
}

/// One frame of session work. Call at the start of every frame, before the UI draws.
pub fn pump(app: &mut App, ctx: &egui::Context) {
    if !app.collab.active() {
        return;
    }
    let local = Local { pointer: ctx.input(|i| i.pointer.hover_pos()), esc: ctx.input(|i| i.key_pressed(egui::Key::Escape)) && app.dialog.is_none() };
    if let Some(wait) = pump_with(app, app.now, &local) {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
    }
}

/// [`pump`] without egui: returns when the next frame is wanted, in seconds.
pub fn pump_with(app: &mut App, now: f64, local: &Local) -> Option<f64> {
    let dt = (now - app.collab.last_now).clamp(0.0, 0.25);
    app.collab.last_now = now;
    let mut wake: f64 = 0.5;
    host_io(app, now, &mut wake);
    guest_io(app, now, &mut wake);

    // presence out
    if app.collab.connected() {
        if let Some(me) = app.collab.self_peer() {
            let cur = local_presence(app, local, me);
            let (bytes, pending) = app.collab.pres.poll(now, &cur);
            if let Some(b) = bytes {
                send_presence(app, &b);
            }
            if pending {
                wake = wake.min(presence::MIN_GAP);
            }
        }
    }

    // other people: ease cursors, drop the silent
    let (gone, moving) = presence::tick(&mut app.collab.remotes, now, dt);
    if moving {
        wake = wake.min(0.016);
    }
    if app.collab.following.is_some_and(|f| gone.contains(&f)) {
        app.collab.following = None;
    }
    follow(app, local.esc, dt, &mut wake);
    Some(wake)
}

fn local_presence(app: &mut App, local: &Local, peer: u32) -> Presence {
    let r = app.canvas_rect;
    let round = |v: f32| (v * 10.0).round() / 10.0;
    let cursor = local.pointer.filter(|p| r.contains(*p)).map(|p| {
        let v = app.view.to_page(r.min, p);
        [round(v.x), round(v.y)]
    });
    let c = app.view.to_page(r.min, r.center());
    Presence {
        peer,
        name: app.collab_name(),
        page: app.page.0,
        cursor,
        sel: app.selection.iter().map(|i| i.0).collect(),
        view: [round(c.x), round(c.y), (app.view.zoom * 1000.0).round() / 1000.0],
        gone: false,
    }
}

fn send_presence(app: &mut App, bytes: &[u8]) {
    if let Some(h) = &app.collab.host {
        for p in h.peers.values() {
            h.link.send_presence(p.net, bytes);
        }
    } else if let Some(g) = app.collab.guest.as_ref().filter(|g| g.live()) {
        g.link.send_presence(bytes);
    }
}

/// True when local edits should be numbered now; otherwise how long until they may be.
fn flush_due(app: &mut App, now: f64, force: bool, wake: &mut f64) -> bool {
    let rev = app.canvas.tiles.rev();
    let p = &mut app.collab.pacing;
    if rev != p.rev || app.canvas.dirty {
        p.rev = rev;
        p.dirty = true;
    }
    if !p.dirty && !force {
        return false;
    }
    let wait = FLUSH_GAP - (now - p.at);
    if wait > 0.0 && !force {
        *wake = wake.min(wait);
        return false;
    }
    p.dirty = false;
    p.at = now;
    true
}

fn host_io(app: &mut App, now: f64, wake: &mut f64) {
    let Some(mut h) = app.collab.host.take() else { return };
    let mut force = false;
    let mut failed = None;
    for ev in h.link.drain() {
        match ev {
            HostEv::Ready { link } => h.url = Some(link),
            HostEv::Failed(e) => failed = Some(e),
            HostEv::Request { id, name } => app.collab.requests.push(Request { id, name }),
            HostEv::Refused { name, error } => app.show_toast(format!("{name} could not join: {}", link::join_error_text(&error)), true),
            HostEv::Joined { peer, name } => {
                // Our own latest edits go to the guests already in; the newcomer gets them in its snapshot.
                let outs = h.sess.local_changes(&app.doc);
                deliver(app, &h, outs);
                // A guest whose connection dropped comes back as the same person, same id range.
                let peers = &h.peers;
                let j = wire::admit(&mut h.sess, &*h.link, &app.doc, peer, h.role, |cp| peers.get(&cp).map(|p| p.net));
                let (cp, role, resumed) = (j.peer, j.role, j.resumed);
                h.peers.insert(cp, Peer { net: peer, name: name.clone(), role });
                // the newcomer sees the guests already here without waiting for their next update
                for r in app.collab.remotes.values() {
                    h.link.send_presence(peer, &r.p.encode());
                }
                app.collab.pres.reset();
                app.show_toast(if resumed { format!("{name} reconnected") } else { format!("{name} joined") }, false);
            }
            HostEv::Left { peer } => {
                if let Some(cp) = h.peers.iter().find(|(_, p)| p.net == peer).map(|(c, _)| *c) {
                    let name = h.peers.remove(&cp).map(|p| p.name).unwrap_or_default();
                    h.sess.remove_peer(cp);
                    app.collab.remotes.remove(&cp);
                    if app.collab.following == Some(cp) {
                        app.collab.following = None;
                    }
                    let bytes = Presence::gone(cp).encode();
                    for p in h.peers.values() {
                        h.link.send_presence(p.net, &bytes);
                    }
                    app.show_toast(format!("{name} left"), false);
                }
            }
            HostEv::Msg { peer, bytes } => {
                let Some(cp) = h.peers.iter().find(|(_, p)| p.net == peer).map(|(c, _)| *c) else { continue };
                match serde_json::from_slice::<ToHost>(&bytes) {
                    Ok(ToHost::Sync(msg)) => {
                        let outs = h.sess.receive(&mut app.doc, cp, msg);
                        deliver(app, &h, outs);
                        app.remote_changed();
                        force = true;
                    }
                    Ok(ToHost::Fonts(keys)) => exchange::answer_fonts(app, &h, cp, keys),
                    Err(_) => {}
                }
            }
            HostEv::Presence { peer, bytes } => {
                let Some(cp) = h.peers.iter().find(|(_, p)| p.net == peer).map(|(c, _)| *c) else { continue };
                let Some(mut p) = Presence::decode(&bytes) else { continue };
                p.peer = cp;
                p.gone = false;
                let out = p.encode();
                wire::relay(&*h.link, cp, &out, h.peers.iter().map(|(c, p)| (*c, p.net)));
                presence::apply(&mut app.collab.remotes, p, now);
            }
        }
    }
    // the address can change while sharing; the Share popover shows the current one
    if app.collab.share.open {
        if let Some(link) = h.link.link() {
            h.url = Some(link);
        }
    }
    if let Some(e) = failed {
        app.show_toast(format!("Sharing stopped: {e}"), true);
        app.collab.remotes.clear();
        app.collab.following = None;
        return;
    }
    if app.history.viewing.is_none() && flush_due(app, now, force, wake) {
        let outs = h.sess.local_changes(&app.doc);
        deliver(app, &h, outs);
    }
    app.collab.host = Some(h);
}

/// Sends the host's outgoing messages, and keeps the host's own undo history from reverting
/// what a guest just changed.
fn deliver(app: &mut App, h: &HostState, outs: Vec<Out>) {
    for out in &outs {
        if let ServerMsg::Ops { from, ops, .. } = &out.msg {
            if *from != 0 {
                for op in ops {
                    rebase_history(&mut app.hist, op);
                }
            }
        }
    }
    wire::route(&*h.link, outs, h.peers.iter().map(|(c, p)| (*c, p.net)));
}

fn guest_io(app: &mut App, now: f64, wake: &mut f64) {
    if app.collab.joining.is_some() {
        joining_io(app, now);
        return;
    }
    let Some(mut g) = app.collab.guest.take() else { return };
    if g.reconnect.is_some() {
        reconnect::io(app, &mut g, now, wake);
        app.collab.guest = Some(g);
        return;
    }
    let mut force = false;
    let mut changed = false;
    for ev in g.link.drain() {
        match ev {
            GuestEv::Joined | GuestEv::Failed(_) => {}
            GuestEv::Closed(c) if g.ended.is_none() && g.reconnect.is_none() => {
                app.collab.remotes.clear();
                app.collab.following = None;
                if reconnect::wanted(&g, &c) {
                    // keep the document, dial again
                    g.reconnect = Some(Reconnect::new(now));
                    g.banner_hidden = false;
                } else {
                    g.ended = Some(c);
                }
            }
            GuestEv::Closed(_) => {}
            GuestEv::Presence(bytes) => {
                if let Some(p) = Presence::decode(&bytes).filter(|p| p.peer != g.sess.peer()) {
                    presence::apply(&mut app.collab.remotes, p, now);
                }
            }
            GuestEv::Msg(bytes) => {
                if !g.live() {
                    continue;
                }
                if let Ok(msg) = serde_json::from_slice::<ToGuest>(&bytes) {
                    changed |= guest_msg(app, &mut g, msg);
                    force = true;
                }
            }
        }
    }
    if changed {
        app.remote_changed();
    }
    if g.live() && app.history.viewing.is_none() && flush_due(app, now, force, wake) {
        if let Some(m) = g.sess.local_changes(&app.doc) {
            g.link.send(encode(&ToHost::Sync(m)));
        }
    }
    app.collab.guest = Some(g);
}

/// Handles one message of the host for a joined guest. Returns whether the document changed.
fn guest_msg(app: &mut App, g: &mut GuestState, msg: ToGuest) -> bool {
    match msg {
        ToGuest::Role(r) => {
            g.role = r;
            false
        }
        ToGuest::Images(list) => {
            g.digests.extend(list.into_iter().map(|i| (i.key, i.digest)));
            false
        }
        ToGuest::Font { key, bytes } => exchange::font_arrived(app, g, key, bytes),
        ToGuest::Sync(msg) => feed(app, g, VecDeque::from([msg])),
    }
}

/// Gives sync messages to the guest session, answers what it asks for (images already in the
/// cache are read from there, not requested), and keeps the undo history off other people's edits.
fn feed(app: &mut App, g: &mut GuestState, mut queue: VecDeque<ServerMsg>) -> bool {
    let mut changed = false;
    while let Some(msg) = queue.pop_front() {
        let Some(msg) = exchange::vet(app, g, msg) else { continue };
        let r = g.sess.receive(&mut app.doc, msg);
        exchange::send_out(app, g, r.out, &mut queue);
        for op in &r.applied {
            rebase_history(&mut app.hist, op);
        }
        changed |= r.changed;
    }
    changed
}

/// Connecting: wait for the verdict and the snapshot.
fn joining_io(app: &mut App, now: f64) {
    let Some(j) = app.collab.joining.as_mut() else { return };
    let mut snapshot = None;
    let mut failed = None;
    let mut early = vec![];
    let mut digests = vec![];
    let mut later = vec![];
    for ev in j.link.drain() {
        match ev {
            GuestEv::Joined => {}
            // arrives with the snapshot; shown once the file is open
            GuestEv::Presence(b) => early.push(b),
            GuestEv::Failed(e) => failed = Some(link::join_error_text(&e)),
            GuestEv::Closed(c) => failed = Some(format!("The connection closed before the file arrived ({c:?}).")),
            GuestEv::Msg(bytes) => match serde_json::from_slice::<ToGuest>(&bytes) {
                Ok(ToGuest::Role(r)) => j.role = r,
                Ok(ToGuest::Images(list)) if snapshot.is_none() => digests.extend(list),
                Ok(ToGuest::Sync(s @ ServerMsg::Snapshot { .. })) if snapshot.is_none() => snapshot = Some(s),
                // sent right behind the snapshot: not to be lost
                Ok(m) if snapshot.is_some() => later.push(m),
                _ => {}
            },
        }
    }
    if let Some(e) = failed {
        app.collab.joining = None;
        if let Some(j) = app.collab.join.as_mut() {
            j.error = Some(e);
            j.connecting = false;
        }
    } else if let Some(s) = snapshot {
        if let Some(j) = app.collab.joining.take() {
            finish_join(app, j, s, digests, later);
            let me = app.collab.self_peer();
            for p in early.iter().filter_map(|b| Presence::decode(b)).filter(|p| Some(p.peer) != me) {
                presence::apply(&mut app.collab.remotes, p, now);
            }
        }
    }
}

/// The snapshot arrived: the shared file replaces whatever the editor had open.
fn finish_join(app: &mut App, j: Joining, snapshot: ServerMsg, digests: Vec<ImageInfo>, later: Vec<ToGuest>) {
    let Some((sess, doc, requests)) = GuestSession::join(snapshot) else {
        j.link.leave();
        if let Some(u) = app.collab.join.as_mut() {
            u.error = Some("The host sent something unexpected. Make sure you both run the same version of desigma.".into());
            u.connecting = false;
        }
        return;
    };
    app.replace_document(doc, None);
    app.home = None;
    app.collab.pacing = Pacing { rev: app.canvas.tiles.rev(), ..Default::default() };
    app.collab.pres.reset();
    let digests = digests.into_iter().map(|i| (i.key, i.digest)).collect();
    let mut g = GuestState {
        link: j.link,
        sess,
        role: j.role,
        ended: None,
        banner_hidden: false,
        redial: j.redial,
        reconnect: None,
        digests,
        fonts: Default::default(),
    };
    let mut queue = VecDeque::new();
    exchange::send_out(app, &mut g, requests, &mut queue);
    let mut changed = feed(app, &mut g, queue);
    for m in later {
        changed |= guest_msg(app, &mut g, m);
    }
    if changed {
        app.remote_changed();
    }
    app.collab.guest = Some(g);
    app.collab.join = None;
    app.show_toast("Joined the session", false);
}

/// Moves the view to the person being followed, until the user pans, zooms or changes page.
fn follow(app: &mut App, esc: bool, dt: f64, wake: &mut f64) {
    let Some(peer) = app.collab.following else { return };
    if esc {
        app.collab.following = None;
        return;
    }
    // The view we set last frame is not ours any more: the user took over.
    if let Some((ox, oy, z, pg)) = app.collab.last_follow {
        let moved = (app.view.offset.x - ox).abs() > 0.5 || (app.view.offset.y - oy).abs() > 0.5 || (app.view.zoom - z).abs() > z * 0.001 || app.page != pg;
        if moved {
            app.collab.following = None;
            app.collab.last_follow = None;
            return;
        }
    }
    let Some(r) = app.collab.remotes.get(&peer) else {
        app.collab.following = None;
        return;
    };
    let (page, view) = (NodeId(r.p.page), r.p.view);
    if page != app.page && app.doc.pages.contains(&page) {
        crate::decks::ui::flush_notes(app);
        app.page = page;
        app.selection.clear();
        app.canvas.reset_interaction();
        app.changed();
    }
    let rect = app.canvas_rect;
    let k = (1.0 - (-dt * 14.0).exp()) as f32;
    let zoom = (app.view.zoom + (view[2].clamp(crate::theme::ZOOM_MIN, crate::theme::ZOOM_MAX) - app.view.zoom) * k)
        .clamp(crate::theme::ZOOM_MIN, crate::theme::ZOOM_MAX);
    let (tx, ty) = (view[0] - rect.width() / 2.0 / zoom, view[1] - rect.height() / 2.0 / zoom);
    let (nx, ny) = (app.view.offset.x + (tx - app.view.offset.x) * k, app.view.offset.y + (ty - app.view.offset.y) * k);
    if (nx - app.view.offset.x).abs() > 0.01 || (ny - app.view.offset.y).abs() > 0.01 || (zoom - app.view.zoom).abs() > 0.0001 {
        *wake = wake.min(0.016);
    }
    app.view.offset = desigma_doc::Vec2::new(nx, ny);
    app.view.zoom = zoom;
    app.collab.last_follow = Some((nx, ny, zoom, app.page));
    app.changed();
}

/// `--panel collab` (screenshot aid): two other people on the canvas, one selecting a layer.
pub fn preview(app: &mut App) {
    let page = app.page.0;
    let first = app.doc.pages.first().and_then(|p| app.doc.get(*p)).and_then(|p| p.children.first().copied()).map(|i| i.0);
    let people = [(1, "Ana", [260.0, 180.0], first.into_iter().collect::<Vec<_>>()), (2, "Boris", [520.0, 340.0], vec![])];
    for (peer, name, cursor, sel) in people {
        let p = Presence { peer, name: name.into(), page, cursor: Some(cursor), sel, view: [0.0, 0.0, 1.0], gone: false };
        presence::apply(&mut app.collab.remotes, p, 0.0);
    }
}
