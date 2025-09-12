use std::{num::NonZeroUsize, sync::Arc};

use lru::LruCache;
use regex::Regex;

/// Default maximum number of cached compiled regexes.
const DEFAULT_CAPACITY: usize = 256;

/// Thread-safe, size-bounded cache for compiled regular expressions.
pub struct RegexCache {
    map: tokio::sync::Mutex<LruCache<String, Arc<Regex>>>,
}

impl Default for RegexCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl RegexCache {
    /// Create a new cache with default capacity.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new cache with a specific capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap();
        Self {
            map: tokio::sync::Mutex::new(LruCache::new(cap)),
        }
    }

    /// Get a compiled regex for `pattern`, compiling and caching on miss.
    pub async fn get_or_compile(&self, pattern: &str) -> Result<Arc<Regex>, regex::Error> {
        // Fast path: try cache
        if let Some(found) = self.map.lock().await.get(pattern).cloned() {
            return Ok(found);
        }

        // Compile outside the lock to avoid blocking other lookups.
        let compiled = Arc::new(Regex::new(pattern)?);

        // Insert, but check again in case another task raced and inserted first.
        let mut guard = self.map.lock().await;
        if let Some(found) = guard.get(pattern).cloned() {
            return Ok(found);
        }
        guard.put(pattern.to_string(), compiled.clone());
        Ok(compiled)
    }
}
