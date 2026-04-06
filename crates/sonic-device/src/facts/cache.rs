use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::Mutex;
use tracing::debug;

// -----------------------------------------------------------------------
// Cache entry
// -----------------------------------------------------------------------

struct CacheEntry {
    /// Serialized JSON representation -- cheap to store, deserialised on get.
    json: String,
    inserted_at: Instant,
}

impl CacheEntry {
    fn is_expired(&self, ttl: Duration) -> bool {
        self.inserted_at.elapsed() > ttl
    }
}

// -----------------------------------------------------------------------
// Public API
// -----------------------------------------------------------------------

/// A TTL-based, type-erased facts cache.
///
/// Values are stored as serialised JSON so that any `Serialize + DeserializeOwned`
/// type can be cached without `Any` / `TypeId` gymnastics.
pub struct FactsCache {
    ttl: Arc<Mutex<Duration>>,
    entries: Arc<Mutex<HashMap<String, CacheEntry>>>,
}

impl FactsCache {
    /// Create a new cache with the given default TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl: Arc::new(Mutex::new(ttl)),
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Retrieve a cached value.  Returns `None` if the key is absent or
    /// expired.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let ttl = *self.ttl.lock().await;
        let entries = self.entries.lock().await;
        let entry = entries.get(key)?;
        if entry.is_expired(ttl) {
            debug!(key, "cache entry expired");
            return None;
        }
        serde_json::from_str(&entry.json).ok()
    }

    /// Insert or overwrite a cache entry.
    pub async fn set<T: Serialize>(&self, key: &str, value: &T) {
        if let Ok(json) = serde_json::to_string(value) {
            let mut entries = self.entries.lock().await;
            entries.insert(
                key.to_string(),
                CacheEntry {
                    json,
                    inserted_at: Instant::now(),
                },
            );
        }
    }

    /// Get a cached value or invoke `fetch_fn` to produce one, caching the
    /// result before returning it.
    pub async fn get_or_fetch<T, F, Fut>(
        &self,
        key: &str,
        fetch_fn: F,
    ) -> sonic_core::Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = sonic_core::Result<T>>,
    {
        if let Some(val) = self.get::<T>(key).await {
            return Ok(val);
        }
        let val = fetch_fn().await?;
        self.set(key, &val).await;
        Ok(val)
    }

    /// Invalidate a single cache key.
    pub async fn invalidate(&self, key: &str) {
        let mut entries = self.entries.lock().await;
        entries.remove(key);
    }

    /// Remove all entries.
    pub async fn invalidate_all(&self) {
        let mut entries = self.entries.lock().await;
        entries.clear();
    }

    /// Change the TTL.  Already-cached entries are evaluated against the new
    /// TTL on the next `get`.
    pub async fn set_ttl(&self, ttl: Duration) {
        let mut current = self.ttl.lock().await;
        *current = ttl;
    }

    /// Number of entries currently in the cache (including potentially
    /// expired ones that haven't been evicted yet).
    pub async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }

    /// Manually evict all expired entries.
    pub async fn evict_expired(&self) {
        let ttl = *self.ttl.lock().await;
        let mut entries = self.entries.lock().await;
        entries.retain(|_, e| !e.is_expired(ttl));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Sample {
        value: u32,
    }

    #[tokio::test]
    async fn test_set_and_get() {
        let cache = FactsCache::new(Duration::from_secs(60));
        let sample = Sample { value: 42 };

        assert!(cache.get::<Sample>("test").await.is_none());

        cache.set("test", &sample).await;
        let retrieved = cache.get::<Sample>("test").await.unwrap();
        assert_eq!(retrieved, sample);
    }

    #[tokio::test]
    async fn test_expiry() {
        let cache = FactsCache::new(Duration::from_millis(50));
        cache.set("test", &Sample { value: 1 }).await;

        // Should still be present.
        assert!(cache.get::<Sample>("test").await.is_some());

        // Wait for expiry.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(cache.get::<Sample>("test").await.is_none());
    }

    #[tokio::test]
    async fn test_invalidate() {
        let cache = FactsCache::new(Duration::from_secs(60));
        cache.set("a", &Sample { value: 1 }).await;
        cache.set("b", &Sample { value: 2 }).await;

        cache.invalidate("a").await;
        assert!(cache.get::<Sample>("a").await.is_none());
        assert!(cache.get::<Sample>("b").await.is_some());

        cache.invalidate_all().await;
        assert!(cache.get::<Sample>("b").await.is_none());
    }

    #[tokio::test]
    async fn test_get_or_fetch() {
        let cache = FactsCache::new(Duration::from_secs(60));

        let val = cache
            .get_or_fetch("key", || async { Ok(Sample { value: 99 }) })
            .await
            .unwrap();
        assert_eq!(val.value, 99);

        // Second call should return the cached value, not call fetch_fn.
        let val2 = cache
            .get_or_fetch("key", || async {
                panic!("should not be called");
            })
            .await
            .unwrap();
        assert_eq!(val2.value, 99);
    }

    #[tokio::test]
    async fn test_set_ttl() {
        let cache = FactsCache::new(Duration::from_secs(60));
        cache.set("test", &Sample { value: 1 }).await;

        // Shorten the TTL to something already expired.
        cache.set_ttl(Duration::from_millis(0)).await;
        assert!(cache.get::<Sample>("test").await.is_none());
    }
}
