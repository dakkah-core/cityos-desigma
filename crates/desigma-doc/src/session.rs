//! Live state of a board kept in the document: the facilitation timer and the voting session.
//! Times are unix seconds, so a running timer survives saving and reopening the file.

use crate::{Document, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Longest timer the widget can reach (minutes).
pub const TIMER_MAX_MINUTES: u32 = 99;
/// Default timer length (minutes).
pub const TIMER_DEFAULT_MINUTES: u32 = 5;
/// Most votes per person a session offers, and the default.
pub const VOTES_MAX: u32 = 20;
pub const VOTES_DEFAULT: u32 = 3;

/// A countdown. While running it ends at `started_at + left_secs`; paused it holds `left_secs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimerState {
    /// Length the timer was set to (what Reset goes back to), seconds.
    pub total_secs: f64,
    /// Seconds left when the timer was last started or paused.
    pub left_secs: f64,
    /// Unix time the timer was started at; `None` when paused.
    pub started_at: Option<f64>,
}

impl TimerState {
    pub fn new(minutes: u32) -> Self {
        let secs = (minutes.clamp(1, TIMER_MAX_MINUTES) * 60) as f64;
        Self { total_secs: secs, left_secs: secs, started_at: None }
    }

    pub fn running(&self) -> bool {
        self.started_at.is_some()
    }

    /// Seconds left at unix time `now` (zero once it has run out).
    pub fn remaining(&self, now: f64) -> f64 {
        match self.started_at {
            Some(t) => (self.left_secs - (now - t)).max(0.0),
            None => self.left_secs.max(0.0),
        }
    }

    pub fn finished(&self, now: f64) -> bool {
        self.remaining(now) <= 0.0
    }

    pub fn start(&mut self, now: f64) {
        if self.started_at.is_none() {
            if self.left_secs <= 0.0 {
                self.left_secs = self.total_secs;
            }
            self.started_at = Some(now);
        }
    }

    pub fn pause(&mut self, now: f64) {
        if self.started_at.is_some() {
            self.left_secs = self.remaining(now);
            self.started_at = None;
        }
    }

    /// One more minute on the clock, running or not.
    pub fn add_minute(&mut self, now: f64) {
        let left = self.remaining(now);
        let cap = (TIMER_MAX_MINUTES * 60) as f64;
        if self.started_at.is_some() {
            self.started_at = Some(now);
        }
        self.left_secs = (left + 60.0).min(cap);
        self.total_secs = self.total_secs.max(self.left_secs);
    }

    /// Back to the set length, stopped.
    pub fn reset(&mut self) {
        self.left_secs = self.total_secs;
        self.started_at = None;
    }

    /// `m:ss` (or `h:mm:ss`) of the time left, rounded up to whole seconds.
    pub fn clock(&self, now: f64) -> String {
        clock_text(self.remaining(now))
    }
}

/// `m:ss`, or `h:mm:ss` from an hour up, of `secs` (rounded up).
pub fn clock_text(secs: f64) -> String {
    let s = secs.max(0.0).ceil() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A voting session: each person has `votes_per_person` votes to spend on nodes. The board is
/// single-user, so `votes` are the votes of the one voter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoteSession {
    pub votes_per_person: u32,
    /// The session has ended: voting is closed and the results are shown.
    pub ended: bool,
    /// Votes by node.
    pub votes: BTreeMap<NodeId, u32>,
}

impl VoteSession {
    pub fn new(votes_per_person: u32) -> Self {
        Self { votes_per_person: votes_per_person.clamp(1, VOTES_MAX), ended: false, votes: BTreeMap::new() }
    }

    pub fn used(&self) -> u32 {
        self.votes.values().sum()
    }

    pub fn left(&self) -> u32 {
        self.votes_per_person.saturating_sub(self.used())
    }

    pub fn count(&self, id: NodeId) -> u32 {
        self.votes.get(&id).copied().unwrap_or(0)
    }

    /// Spends one vote on `id`; false when voting is closed or no votes are left.
    pub fn add(&mut self, id: NodeId) -> bool {
        if self.ended || self.left() == 0 {
            return false;
        }
        *self.votes.entry(id).or_insert(0) += 1;
        true
    }

    /// Takes one vote back from `id`; false when it has none or voting is closed.
    pub fn remove(&mut self, id: NodeId) -> bool {
        if self.ended {
            return false;
        }
        match self.votes.get_mut(&id) {
            Some(n) if *n > 1 => *n -= 1,
            Some(_) => {
                self.votes.remove(&id);
            }
            None => return false,
        }
        true
    }

    /// Nodes of `doc` that got votes, most votes first (ties in document id order).
    pub fn results(&self, doc: &Document) -> Vec<(NodeId, u32)> {
        let mut r: Vec<(NodeId, u32)> = self.votes.iter().filter(|(id, n)| **n > 0 && doc.get(**id).is_some()).map(|(id, n)| (*id, *n)).collect();
        r.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        r
    }
}

/// The timer and the vote of a board; stored in the document and edited with
/// [`crate::Command::SetBoardSession`] so every change is undoable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoardSession {
    pub timer: Option<TimerState>,
    pub vote: Option<VoteSession>,
}

impl BoardSession {
    /// Voting is open.
    pub fn voting(&self) -> bool {
        self.vote.as_ref().is_some_and(|v| !v.ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_counts_down_pauses_and_adds_minutes() {
        let mut t = TimerState::new(2);
        assert_eq!(t.remaining(1000.0), 120.0);
        t.start(1000.0);
        assert!(t.running());
        assert_eq!(t.remaining(1030.0), 90.0);
        assert_eq!(t.clock(1030.5), "1:30");
        t.pause(1030.0);
        assert_eq!(t.remaining(5000.0), 90.0);
        t.add_minute(5000.0);
        assert_eq!(t.remaining(5000.0), 150.0);
        t.start(5000.0);
        t.add_minute(5010.0);
        assert_eq!(t.remaining(5010.0), 200.0);
        assert!(t.finished(6000.0));
        assert_eq!(t.remaining(6000.0), 0.0);
        t.pause(6000.0);
        t.start(6000.0);
        assert!(t.remaining(6000.0) > 0.0, "starting a finished timer restarts it");
        t.reset();
        assert!(!t.running() && t.remaining(0.0) == t.total_secs);
        assert_eq!(clock_text(3725.0), "1:02:05");
        assert_eq!(clock_text(0.2), "0:01");
    }

    #[test]
    fn votes_are_limited_and_ranked() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let ids: Vec<NodeId> = (0..3)
            .map(|_| {
                let id = d.alloc_id();
                d.insert(page, crate::Node::sticky(id, crate::Rect::new(0.0, 0.0, 10.0, 10.0), "")).unwrap()
            })
            .collect();
        let mut v = VoteSession::new(3);
        assert!(v.add(ids[1]) && v.add(ids[0]) && v.add(ids[1]));
        assert!(!v.add(ids[2]), "out of votes");
        assert_eq!((v.used(), v.left(), v.count(ids[1])), (3, 0, 2));
        assert_eq!(v.results(&d), vec![(ids[1], 2), (ids[0], 1)]);
        assert!(v.remove(ids[0]) && !v.remove(ids[0]));
        assert!(v.add(ids[2]));
        v.ended = true;
        assert!(!v.add(ids[0]) && !v.remove(ids[1]));
        d.remove(ids[1]).unwrap();
        assert_eq!(v.results(&d), vec![(ids[2], 1)]);
    }

    #[test]
    fn session_edits_are_undoable_and_saved_with_the_document() {
        let mut d = Document::new("t");
        d.add_page("p");
        let mut h = crate::History::default();
        let mut s = BoardSession { timer: Some(TimerState::new(5)), ..Default::default() };
        h.exec(&mut d, crate::Command::SetBoardSession { session: s.clone() }).unwrap();
        s.vote = Some(VoteSession::new(3));
        h.exec(&mut d, crate::Command::SetBoardSession { session: s.clone() }).unwrap();
        assert!(d.board_session.voting());
        let back = Document::from_json(&d.to_json().unwrap()).unwrap();
        assert_eq!(back.board_session, d.board_session);
        h.undo(&mut d).unwrap();
        assert!(d.board_session.vote.is_none() && d.board_session.timer.is_some());
        h.undo(&mut d).unwrap();
        assert_eq!(d.board_session, BoardSession::default());
        h.redo(&mut d).unwrap();
        assert!(d.board_session.timer.is_some());
        // files from before the session existed load with an empty one
        let mut v: serde_json::Value = serde_json::from_str(&d.to_json().unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("board_session");
        let old = Document::from_json(&v.to_string()).unwrap();
        assert_eq!(old.board_session, BoardSession::default());
    }

    #[test]
    fn session_round_trips_and_defaults() {
        let s: BoardSession = serde_json::from_str("{}").unwrap();
        assert_eq!(s, BoardSession::default());
        let mut s = BoardSession { timer: Some(TimerState::new(3)), vote: Some(VoteSession::new(5)) };
        s.vote.as_mut().unwrap().add(NodeId(7));
        let back: BoardSession = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert!(back.voting());
    }
}
