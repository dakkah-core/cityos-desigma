//! Password check and failed-attempt rate limiting.
use iroh::EndpointId;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Constant-time equality (the length is not hidden; passwords are short).
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

struct Entry {
    fails: u32,
    last: Instant,
    locked_until: Option<Instant>,
}

/// Failed-password tracker per remote endpoint id: a growing delay, then a lockout.
pub struct Limiter {
    max_fails: u32,
    lockout: Duration,
    map: HashMap<EndpointId, Entry>,
}

impl Limiter {
    pub fn new(max_fails: u32, lockout: Duration) -> Self {
        Self { max_fails, lockout, map: HashMap::new() }
    }

    /// `Err(remaining)` while the endpoint is locked out.
    pub fn check(&mut self, id: &EndpointId) -> Result<(), Duration> {
        let now = Instant::now();
        if let Some(e) = self.map.get_mut(id) {
            match e.locked_until {
                Some(t) if t > now => return Err(t - now),
                Some(_) => {
                    e.locked_until = None;
                    e.fails = 0;
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Record a failure; returns how long to stall the answer.
    pub fn fail(&mut self, id: EndpointId) -> Duration {
        let now = Instant::now();
        if self.map.len() > 1024 {
            self.map.retain(|_, e| now.duration_since(e.last) < Duration::from_secs(600));
        }
        let e = self.map.entry(id).or_insert(Entry { fails: 0, last: now, locked_until: None });
        e.fails += 1;
        e.last = now;
        if e.fails >= self.max_fails {
            e.locked_until = Some(now + self.lockout);
        }
        Duration::from_millis(250u64 << (e.fails - 1).min(4)).min(Duration::from_secs(4))
    }

    pub fn success(&mut self, id: &EndpointId) {
        self.map.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares() {
        assert!(ct_eq("hunter2", "hunter2"));
        assert!(!ct_eq("hunter2", "hunter3"));
        assert!(!ct_eq("hunter2", "hunter"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn locks_out_after_max_and_resets() {
        let id = iroh::SecretKey::generate().public();
        let mut l = Limiter::new(3, Duration::from_secs(60));
        assert!(l.check(&id).is_ok());
        let d1 = l.fail(id);
        let d2 = l.fail(id);
        assert!(d2 > d1);
        assert!(l.check(&id).is_ok());
        l.fail(id);
        assert!(l.check(&id).is_err());
        l.success(&id);
        assert!(l.check(&id).is_ok());
    }
}
