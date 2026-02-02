//! Caching utilities for Cortex.

use std::collections::HashMap;
use std::hash::Hash;

/// Simple in-memory LRU cache.
pub struct LruCache<K, V> {
    capacity: usize,
    items: HashMap<K, V>,
    order: Vec<K>,
}

impl<K: Eq + Hash + Clone, V> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: HashMap::with_capacity(capacity),
            order: Vec::with_capacity(capacity),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.items.contains_key(key) {
            self.order.retain(|k| k != key);
            self.order.push(key.clone());
            self.items.get(key)
        } else {
            None
        }
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.items.len() >= self.capacity
            && !self.items.contains_key(&key)
            && let Some(oldest) = self.order.first().cloned()
        {
            self.items.remove(&oldest);
            self.order.remove(0);
        }

        self.order.retain(|k| k != &key);
        self.order.push(key.clone());
        self.items.insert(key, value);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
