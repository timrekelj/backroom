//! A small in-memory map whose entries expire, for short-lived auth state
//! (second-factor challenges, pending TOTP secrets, passkey ceremonies).
//!
//! Single-process like the rate limiter: with several server instances a
//! ceremony must finish on the instance that started it (sticky sessions).
//! Entries are lost on restart, which only means starting the step again.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Upper bound on live entries; inserting past it drops expired entries and
/// then the entries closest to expiry.
const MAX_ENTRIES: usize = 20_000;

pub struct TtlMap<K, V> {
    entries: Mutex<HashMap<K, (Instant, V)>>,
    ttl: Duration,
}

impl<K: Eq + Hash + Clone, V: Clone> TtlMap<K, V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<K, (Instant, V)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, key: K, value: V) {
        let now = Instant::now();
        let mut entries = self.lock();
        if entries.len() >= MAX_ENTRIES {
            entries.retain(|_, (at, _)| *at > now);
            while entries.len() >= MAX_ENTRIES {
                let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                entries.remove(&oldest);
            }
        }
        entries.insert(key, (now + self.ttl, value));
    }

    /// The live value of `key`.
    pub fn get(&self, key: &K) -> Option<V> {
        let now = Instant::now();
        let mut entries = self.lock();
        match entries.get(key) {
            Some((at, v)) if *at > now => Some(v.clone()),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    /// Changes a live value in place (keeping its expiry); returns the result of `f`.
    pub fn update<R>(&self, key: &K, f: impl FnOnce(&mut V) -> R) -> Option<R> {
        let now = Instant::now();
        let mut entries = self.lock();
        match entries.get_mut(key) {
            Some((at, v)) if *at > now => Some(f(v)),
            _ => None,
        }
    }

    /// Removes and returns the live value of `key`.
    pub fn take(&self, key: &K) -> Option<V> {
        let now = Instant::now();
        self.lock()
            .remove(key)
            .filter(|(at, _)| *at > now)
            .map(|(_, v)| v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_expire_and_take_removes() {
        let map = TtlMap::new(Duration::from_millis(40));
        map.insert("a", 1);
        assert_eq!(map.get(&"a"), Some(1));
        assert_eq!(
            map.update(&"a", |v| {
                *v += 1;
                *v
            }),
            Some(2)
        );
        assert_eq!(map.take(&"a"), Some(2));
        assert_eq!(map.get(&"a"), None);
        map.insert("b", 1);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(map.get(&"b"), None);
    }
}
