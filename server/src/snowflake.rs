//! Discord-style Snowflake IDs: 41 bits ms since the custom epoch | 10 bits worker | 12 bits sequence.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 2026-01-01T00:00:00Z in milliseconds since the Unix epoch.
pub const EPOCH_MS: u64 = 1_767_225_600_000;
pub const WORKER_BITS: u32 = 10;
pub const SEQUENCE_BITS: u32 = 12;
pub const MAX_WORKER_ID: u16 = (1 << WORKER_BITS) - 1;
const MAX_SEQUENCE: u64 = (1 << SEQUENCE_BITS) - 1;
const TIMESTAMP_SHIFT: u32 = WORKER_BITS + SEQUENCE_BITS;

/// Milliseconds per message bucket (10 days).
pub const BUCKET_MS: u64 = 10 * 24 * 60 * 60 * 1000;

#[derive(Debug)]
pub struct Snowflake {
    worker_id: u64,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    last_ms: u64,
    sequence: u64,
}

impl Snowflake {
    pub fn new(worker_id: u16) -> Self {
        assert!(worker_id <= MAX_WORKER_ID, "worker id must fit in 10 bits");
        Self {
            worker_id: worker_id as u64,
            state: Mutex::new(State {
                last_ms: 0,
                sequence: 0,
            }),
        }
    }

    /// Generates the next unique, increasing ID.
    pub fn next_id(&self) -> u64 {
        loop {
            let now = now_ms();
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // If the clock went backwards, keep issuing from the last timestamp.
            let ms = now.max(st.last_ms);
            if ms == st.last_ms {
                if st.sequence == MAX_SEQUENCE {
                    // Sequence exhausted for this ms: wait for the clock to move on.
                    drop(st);
                    std::thread::yield_now();
                    continue;
                }
                st.sequence += 1;
            } else {
                st.last_ms = ms;
                st.sequence = 0;
            }
            return (ms << TIMESTAMP_SHIFT) | (self.worker_id << SEQUENCE_BITS) | st.sequence;
        }
    }
}

/// Milliseconds since the Snowflake epoch according to the wall clock.
fn now_ms() -> u64 {
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64;
    unix_ms.saturating_sub(EPOCH_MS)
}

/// Milliseconds since the Snowflake epoch encoded in `id`.
pub fn millis_of(id: u64) -> u64 {
    id >> TIMESTAMP_SHIFT
}

pub fn timestamp_of(id: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(EPOCH_MS + millis_of(id))
}

pub fn worker_of(id: u64) -> u16 {
    ((id >> SEQUENCE_BITS) & MAX_WORKER_ID as u64) as u16
}

pub fn sequence_of(id: u64) -> u16 {
    (id & MAX_SEQUENCE) as u16
}

/// Builds an ID from its parts; `ms_since_epoch` is relative to [`EPOCH_MS`].
pub fn make_id(ms_since_epoch: u64, worker: u16, sequence: u16) -> u64 {
    assert!(worker <= MAX_WORKER_ID, "worker id must fit in 10 bits");
    assert!(sequence as u64 <= MAX_SEQUENCE, "sequence must fit in 12 bits");
    (ms_since_epoch << TIMESTAMP_SHIFT) | ((worker as u64) << SEQUENCE_BITS) | sequence as u64
}

/// Message bucket (10-day window since the epoch) that `id` falls into.
pub fn bucket_of(id: u64) -> i32 {
    (millis_of(id) / BUCKET_MS) as i32
}

/// Message bucket of the current wall-clock time, without consuming an ID.
pub fn current_bucket() -> i32 {
    (now_ms() / BUCKET_MS) as i32
}

/// Smallest ID belonging to `bucket`.
pub fn min_id_for_bucket(bucket: i32) -> u64 {
    (bucket.max(0) as u64 * BUCKET_MS) << TIMESTAMP_SHIFT
}

/// Largest ID belonging to `bucket`.
pub fn max_id_for_bucket(bucket: i32) -> u64 {
    min_id_for_bucket(bucket + 1) - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[test]
    fn monotonic_and_unique_over_100k() {
        let g = Snowflake::new(3);
        let mut prev = 0;
        let mut seen = HashSet::new();
        for _ in 0..100_000 {
            let id = g.next_id();
            assert!(id > prev);
            assert!(seen.insert(id));
            prev = id;
        }
    }

    #[test]
    fn unique_across_threads() {
        let g = Arc::new(Snowflake::new(1));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let g = g.clone();
                std::thread::spawn(move || (0..25_000).map(|_| g.next_id()).collect::<Vec<_>>())
            })
            .collect();
        let mut seen = HashSet::new();
        for h in handles {
            for id in h.join().unwrap() {
                assert!(seen.insert(id));
            }
        }
        assert_eq!(seen.len(), 100_000);
    }

    #[test]
    fn bit_layout_round_trip() {
        let g = Snowflake::new(MAX_WORKER_ID);
        let before = SystemTime::now();
        let id = g.next_id();
        let after = SystemTime::now();
        assert_eq!(worker_of(id), MAX_WORKER_ID);
        assert!(id < 1 << 63, "must fit in i64");
        let ts = timestamp_of(id);
        assert!(ts >= before - Duration::from_millis(1) && ts <= after + Duration::from_millis(1));
        let composed = (millis_of(id) << 22) | ((worker_of(id) as u64) << 12) | sequence_of(id) as u64;
        assert_eq!(composed, id);
    }

    #[test]
    fn bucket_math() {
        assert_eq!(bucket_of(0), 0);
        assert_eq!(bucket_of(min_id_for_bucket(1) - 1), 0);
        assert_eq!(bucket_of(min_id_for_bucket(1)), 1);
        assert_eq!(bucket_of(max_id_for_bucket(5)), 5);
        assert_eq!(bucket_of(max_id_for_bucket(5) + 1), 6);
        assert_eq!(min_id_for_bucket(0), 0);
    }

    #[test]
    fn make_id_round_trips() {
        let id = make_id(BUCKET_MS * 3 + 17, 5, 99);
        assert_eq!(millis_of(id), BUCKET_MS * 3 + 17);
        assert_eq!(worker_of(id), 5);
        assert_eq!(sequence_of(id), 99);
        assert_eq!(bucket_of(id), 3);
    }

    #[test]
    fn current_bucket_matches_fresh_id() {
        let id = Snowflake::new(0).next_id();
        assert_eq!(current_bucket(), bucket_of(id));
    }

    #[test]
    fn epoch_is_2026_01_01() {
        assert_eq!(EPOCH_MS, 1_767_225_600_000);
    }
}
