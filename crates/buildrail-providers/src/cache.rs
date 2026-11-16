//! A tiny bounded in-memory cache for conditional (`ETag`) requests.
//!
//! GitHub does not count `304 Not Modified` answers against the rate limit,
//! so remembering the last `ETag` per request lets the refresh engine poll
//! often for free. The cache stores the *parsed* result, so a 304 costs no
//! decoding either.

use std::collections::HashMap;
use std::sync::Mutex;

/// Bounded map from a request key to `(etag, value)`. When full, the least
/// recently stored entry is evicted. Thread-safe; the lock is never held
/// across an `.await`.
pub(crate) struct EtagCache<K, V> {
    inner: Mutex<Inner<K, V>>,
    capacity: usize,
}

struct Inner<K, V> {
    entries: HashMap<K, Entry<V>>,
    tick: u64,
}

struct Entry<V> {
    etag: String,
    value: V,
    stored: u64,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> EtagCache<K, V> {
    /// An empty cache holding at most `capacity` entries (minimum 1).
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                tick: 0,
            }),
            capacity: capacity.max(1),
        }
    }

    /// The cached `(etag, value)` for `key`.
    pub(crate) fn get(&self, key: &K) -> Option<(String, V)> {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner
            .entries
            .get(key)
            .map(|e| (e.etag.clone(), e.value.clone()))
    }

    /// Stores (or replaces) the entry for `key`.
    pub(crate) fn put(&self, key: K, etag: String, value: V) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.tick += 1;
        let stored = inner.tick;
        if !inner.entries.contains_key(&key) && inner.entries.len() >= self.capacity {
            let oldest = inner
                .entries
                .iter()
                .min_by_key(|(_, e)| e.stored)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                inner.entries.remove(&oldest);
            }
        }
        inner.entries.insert(
            key,
            Entry {
                etag,
                value,
                stored,
            },
        );
    }

    /// Drops the entry for `key`, if any.
    pub(crate) fn remove(&self, key: &K) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.entries.remove(key);
    }

    /// Number of entries (for `Debug` output and tests).
    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entries
            .len()
    }
}

impl<K, V> std::fmt::Debug for EtagCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EtagCache")
            .field("entries", &self.len())
            .field("capacity", &self.capacity)
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_when_full() {
        let cache = EtagCache::new(2);
        cache.put("a", "1".into(), 1);
        cache.put("b", "2".into(), 2);
        cache.put("a", "3".into(), 3); // replace keeps size
        cache.put("c", "4".into(), 4); // evicts "b", the oldest
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"a"), Some(("3".into(), 3)));
        assert_eq!(cache.get(&"c"), Some(("4".into(), 4)));
        cache.remove(&"a");
        assert_eq!(cache.len(), 1);
        assert!(format!("{cache:?}").contains("entries: 1"));
    }
}
