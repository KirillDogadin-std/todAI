//! Locks out addresses that keep sending wrong tokens. A 256-bit token can't
//! be guessed anyway; this keeps a misbehaving client (or a scanner) from
//! burning threads and filling logs, and makes guessing visibly pointless.
//!
//! The policy is plain data ([`Policy`]) so it can be tuned, or swapped for
//! a different scheme, without touching the routes: `http.rs` only asks
//! [`Limiter::check`] before comparing tokens and reports the outcome.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Wrong tokens an address may send before it is locked out.
    pub failures: u32,
    /// The first lockout; each further one doubles, up to `max_lockout`.
    pub lockout: Duration,
    pub max_lockout: Duration,
    /// Failures older than this are forgotten.
    pub window: Duration,
    /// Addresses tracked at most; past this, the stalest are dropped.
    pub capacity: usize,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            failures: 20,
            lockout: Duration::from_secs(60),
            max_lockout: Duration::from_secs(3600),
            window: Duration::from_secs(600),
            capacity: 4096,
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    failures: u32,
    /// Lockouts so far, for the doubling.
    strikes: u32,
    last: Instant,
    locked_until: Option<Instant>,
}

pub struct Limiter {
    policy: Policy,
    seen: Mutex<HashMap<IpAddr, Entry>>,
}

impl Limiter {
    pub fn new(policy: Policy) -> Limiter {
        Limiter { policy, seen: Mutex::new(HashMap::new()) }
    }

    /// `Err(seconds left)` while `ip` is locked out; its token isn't even
    /// looked at then, so a right guess during a lockout doesn't count.
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        let now = Instant::now();
        let seen = self.seen.lock().unwrap();
        match seen.get(&ip).and_then(|e| e.locked_until) {
            Some(until) if until > now => Err((until - now).as_secs().max(1)),
            _ => Ok(()),
        }
    }

    /// A right token: the address starts over (lockout history included).
    pub fn success(&self, ip: IpAddr) {
        self.seen.lock().unwrap().remove(&ip);
    }

    /// A wrong or missing token.
    pub fn failure(&self, ip: IpAddr) {
        let now = Instant::now();
        let p = self.policy;
        let mut seen = self.seen.lock().unwrap();
        if seen.len() >= p.capacity && !seen.contains_key(&ip) {
            evict(&mut seen, now, p);
        }
        let e = seen.entry(ip).or_insert(Entry { failures: 0, strikes: 0, last: now, locked_until: None });
        if now.duration_since(e.last) > p.window {
            e.failures = 0;
        }
        e.last = now;
        e.failures += 1;
        if e.failures >= p.failures {
            let factor = 1u32 << e.strikes.min(16);
            e.locked_until = Some(now + p.lockout.saturating_mul(factor).min(p.max_lockout));
            e.strikes += 1;
            e.failures = 0;
        }
    }
}

/// Drops entries that are neither locked nor recent, else the stalest one.
fn evict(seen: &mut HashMap<IpAddr, Entry>, now: Instant, p: Policy) {
    seen.retain(|_, e| e.locked_until.is_some_and(|u| u > now) || now.duration_since(e.last) <= p.window);
    if seen.len() >= p.capacity
        && let Some(ip) = seen.iter().min_by_key(|(_, e)| e.last).map(|(ip, _)| *ip)
    {
        seen.remove(&ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy { failures: 3, lockout: Duration::from_secs(60), ..Policy::default() }
    }

    #[test]
    fn locks_out_after_enough_failures_and_doubles() {
        let l = Limiter::new(policy());
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..2 {
            l.failure(ip);
            assert!(l.check(ip).is_ok());
        }
        l.failure(ip);
        let left = l.check(ip).unwrap_err();
        assert!((59..=60).contains(&left), "{left}");
        // Another round while still counted doubles the lockout.
        for _ in 0..3 {
            l.failure(ip);
        }
        assert!(l.check(ip).unwrap_err() > 60);
        assert!(l.check("10.0.0.2".parse().unwrap()).is_ok(), "other addresses are unaffected");
    }

    #[test]
    fn success_resets() {
        let l = Limiter::new(policy());
        let ip: IpAddr = "::1".parse().unwrap();
        l.failure(ip);
        l.failure(ip);
        l.success(ip);
        l.failure(ip);
        assert!(l.check(ip).is_ok());
    }

    #[test]
    fn capacity_is_bounded() {
        let l = Limiter::new(Policy { capacity: 4, ..policy() });
        for i in 0..20u8 {
            l.failure(IpAddr::from([10, 0, 0, i]));
        }
        assert!(l.seen.lock().unwrap().len() <= 4);
    }
}
