//! In-memory log of recent sends (PLAN.md Phase 1), for debugging before history
//! exists (Phase 5).
//!
//! Entries must already be redacted: build URLs with [`crate::redact::redact_url`].

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Thread-safe ring buffer of the last [`RecentSends::CAPACITY`] entries.
#[derive(Debug)]
pub struct RecentSends<T> {
    entries: Mutex<VecDeque<T>>,
}

impl<T: Clone> RecentSends<T> {
    pub const CAPACITY: usize = 50;

    pub fn new() -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(Self::CAPACITY)),
        }
    }

    /// Adds an entry, dropping the oldest one when full.
    pub fn push(&self, entry: T) {
        let mut entries = self.entries.lock().expect("recent sends lock poisoned");
        if entries.len() == Self::CAPACITY {
            entries.pop_back();
        }
        entries.push_front(entry);
    }

    /// Replaces the newest entry matching `pred`, e.g. to fill in a result.
    pub fn update(&self, pred: impl Fn(&T) -> bool, f: impl FnOnce(&mut T)) {
        let mut entries = self.entries.lock().expect("recent sends lock poisoned");
        if let Some(entry) = entries.iter_mut().find(|e| pred(e)) {
            f(entry);
        }
    }

    /// All entries, newest first.
    pub fn list(&self) -> Vec<T> {
        self.entries
            .lock()
            .expect("recent sends lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

impl<T: Clone> Default for RecentSends<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Formats a time as an RFC 3339 UTC timestamp with milliseconds,
/// e.g. `2026-10-05T05:28:20.151Z`.
pub fn rfc3339_utc(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn keeps_newest_first_and_caps() {
        let log = RecentSends::new();
        for i in 0..60 {
            log.push(i);
        }
        let list = log.list();
        assert_eq!(list.len(), RecentSends::<i32>::CAPACITY);
        assert_eq!(list[0], 59);
        assert_eq!(*list.last().unwrap(), 10);
    }

    #[test]
    fn update_changes_matching_entry() {
        let log = RecentSends::new();
        log.push((1, None));
        log.push((2, None));
        log.update(|e| e.0 == 1, |e| e.1 = Some(200));
        assert_eq!(log.list(), vec![(2, None), (1, Some(200))]);
    }

    #[test]
    fn formats_rfc3339() {
        assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let t = UNIX_EPOCH + Duration::from_millis(1_791_178_100_151);
        assert_eq!(rfc3339_utc(t), "2026-10-05T05:28:20.151Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(rfc3339_utc(leap), "2000-02-29T00:00:00.000Z");
    }
}
