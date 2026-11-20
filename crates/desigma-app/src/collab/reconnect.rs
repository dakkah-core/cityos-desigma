//! A guest whose connection dropped keeps its document (still editable) and dials the host again
//! with the secret it was given, as the same peer. On success it gets a fresh snapshot and
//! [`GuestSession::rejoin`](desigma_collab::GuestSession::rejoin) lays the edits made meanwhile
//! over it, so nothing is lost. After [`WINDOW`] seconds it gives up and the file is a local copy.

use super::link::{Closed, GuestEv, GuestLink};
use super::{exchange, feed, guest_msg, GuestState, ToGuest};
use crate::app::App;
use desigma_collab::{rebase_history, Role, ServerMsg};
use desigma_net::JoinError;
use std::collections::VecDeque;

/// How long to keep trying, from the moment the connection was lost.
pub const WINDOW: f64 = 120.0;
/// An attempt that has not produced the snapshot by now is abandoned for the next one.
const ATTEMPT: f64 = 30.0;
/// Waits before the first attempts; after these, every [`STEADY`] seconds.
const DELAYS: [f64; 4] = [1.0, 2.0, 5.0, 10.0];
const STEADY: f64 = 15.0;

/// Seconds to wait before attempt number `attempt` (0 is the first).
pub fn delay(attempt: usize) -> f64 {
    DELAYS.get(attempt).copied().unwrap_or(STEADY)
}

pub struct Reconnect {
    since: f64,
    attempt: usize,
    next: f64,
    dial: Option<Dial>,
}

/// One attempt in progress.
struct Dial {
    link: Box<dyn GuestLink>,
    started: f64,
    role: Role,
    snapshot: bool,
    /// Messages that came right behind the snapshot.
    later: Vec<ToGuest>,
}

impl Reconnect {
    pub fn new(now: f64) -> Self {
        Self { since: now, attempt: 0, next: now + delay(0), dial: None }
    }

    /// Seconds until the window closes.
    pub fn left(&self, now: f64) -> f64 {
        (WINDOW - (now - self.since)).max(0.0)
    }

    fn retry(&mut self, now: f64) {
        self.attempt += 1;
        self.next = now + delay(self.attempt);
        self.dial = None;
    }
}

/// Whether a closed connection should be redialled rather than end the session: only a lost one,
/// and only when we can dial and hold the host's secret.
pub fn wanted(g: &GuestState, c: &Closed) -> bool {
    matches!(c, Closed::Lost(_)) && g.redial.is_some() && g.sess.token().is_some()
}

/// Refusals that trying again cannot fix.
fn fatal(e: &JoinError) -> bool {
    matches!(e, JoinError::WrongPassword | JoinError::Denied | JoinError::VersionMismatch { .. })
}

fn give_up(g: &mut GuestState, why: &str) {
    g.ended = Some(Closed::Lost(why.to_string()));
    g.reconnect = None;
    g.banner_hidden = false;
}

/// One frame of reconnecting.
pub fn io(app: &mut App, g: &mut GuestState, now: f64, wake: &mut f64) {
    let Some(mut rc) = g.reconnect.take() else { return };
    if rc.left(now) <= 0.0 {
        if let Some(d) = rc.dial.take() {
            d.link.leave();
        }
        give_up(g, "could not reconnect");
        return;
    }
    *wake = wake.min(rc.left(now));
    if let Some(mut d) = rc.dial.take() {
        let mut snapshot = None;
        let mut failure: Option<bool> = None;
        for ev in d.link.drain() {
            match ev {
                GuestEv::Joined | GuestEv::Presence(_) => {}
                GuestEv::Failed(e) => failure = Some(fatal(&e)),
                GuestEv::Closed(_) => {
                    failure.get_or_insert(false);
                }
                GuestEv::Msg(bytes) => match serde_json::from_slice::<ToGuest>(&bytes) {
                    Ok(ToGuest::Role(r)) => d.role = r,
                    Ok(ToGuest::Sync(s @ ServerMsg::Snapshot { .. })) if !d.snapshot => {
                        d.snapshot = true;
                        snapshot = Some(s);
                    }
                    Ok(m) if d.snapshot => d.later.push(m),
                    Ok(ToGuest::Images(list)) => g.digests.extend(list.into_iter().map(|i| (i.key, i.digest))),
                    _ => {}
                },
            }
        }
        if let Some(s) = snapshot {
            complete(app, g, d, s);
            return;
        }
        match failure {
            Some(true) => {
                d.link.leave();
                give_up(g, "the host refused the connection");
                return;
            }
            Some(false) => {
                d.link.leave();
                rc.retry(now);
            }
            None if now - d.started > ATTEMPT => {
                d.link.leave();
                rc.retry(now);
            }
            None => {
                *wake = wake.min(0.25);
                rc.dial = Some(d);
            }
        }
    } else if now >= rc.next {
        let token = g.sess.token().map(str::to_string);
        match g.redial.as_ref().map(|dial| dial(token.as_deref())) {
            Some(Ok(link)) => {
                rc.dial = Some(Dial { link, started: now, role: g.role, snapshot: false, later: Vec::new() });
                *wake = wake.min(0.25);
            }
            _ => rc.retry(now),
        }
    } else {
        *wake = wake.min(rc.next - now);
    }
    g.reconnect = Some(rc);
}

/// The host sent its fresh snapshot: lay the work done offline over it and carry on.
fn complete(app: &mut App, g: &mut GuestState, d: Dial, snapshot: ServerMsg) {
    let Some(r) = g.sess.rejoin(&mut app.doc, snapshot) else {
        d.link.leave();
        give_up(g, "the host does not know this connection");
        return;
    };
    g.link = d.link;
    g.role = d.role;
    g.banner_hidden = false;
    for op in &r.applied {
        rebase_history(&mut app.hist, op);
    }
    let mut changed = r.changed;
    let mut queue = VecDeque::new();
    exchange::send_out(app, g, r.out, &mut queue);
    changed |= feed(app, g, queue);
    for m in d.later {
        changed |= guest_msg(app, g, m);
    }
    if changed {
        app.remote_changed();
    }
    // send what the host is still missing at once
    app.collab.pacing.dirty = true;
    app.collab.pacing.at = f64::NEG_INFINITY;
    app.collab.pres.reset();
    app.show_toast("Reconnected", false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_off_one_two_five_ten_then_every_fifteen_seconds() {
        let got: Vec<f64> = (0..7).map(delay).collect();
        assert_eq!(got, vec![1.0, 2.0, 5.0, 10.0, 15.0, 15.0, 15.0]);
        let mut rc = Reconnect::new(100.0);
        assert_eq!(rc.next, 101.0);
        rc.retry(101.0);
        assert_eq!(rc.next, 103.0);
        assert_eq!(rc.left(100.0 + WINDOW + 1.0), 0.0);
    }
}
