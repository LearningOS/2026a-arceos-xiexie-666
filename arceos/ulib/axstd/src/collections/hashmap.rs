//! A minimal hash map built on an open addressing table.
//!
//! `alloc` only provides tree based collections, so a `HashMap` is implemented
//! here. It uses linear probing over a power-of-two sized table of buckets and
//! doubles the table whenever the load factor would exceed 3/4.

use alloc::vec::Vec;
use core::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};

/// A hasher builder that always creates the same [`FnvHasher`].
///
/// Hash randomisation is not needed in a kernel: the keys of an in-memory map
/// come from the program itself, so the cheapest possible hasher is preferable.
pub type HashMapBuilder = BuildHasherDefault<FnvHasher>;

/// The 64-bit Fowler-Noll-Vo hash function (FNV-1a).
#[derive(Clone, Copy)]
pub struct FnvHasher(u64);

impl Default for FnvHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// A hash map implemented with linear probing and a power-of-two table.
pub struct HashMap<K, V, S = HashMapBuilder> {
    buckets: Vec<Option<(K, V)>>,
    len: usize,
    hasher: S,
}

impl<K, V> HashMap<K, V, HashMapBuilder> {
    /// Creates an empty `HashMap`.
    pub fn new() -> Self {
        Self {
            buckets: Vec::new(),
            len: 0,
            hasher: BuildHasherDefault::default(),
        }
    }
}

impl<K, V> Default for HashMap<K, V, HashMapBuilder> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S> HashMap<K, V, S> {
    /// Creates an empty `HashMap` which will use the given hasher builder.
    pub fn with_hasher(hasher: S) -> Self {
        Self {
            buckets: Vec::new(),
            len: 0,
            hasher,
        }
    }

    /// Returns the number of elements in the map.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the map contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes all key-value pairs from the map, keeping the allocated space.
    pub fn clear(&mut self) {
        self.buckets.clear();
        self.len = 0;
    }

    /// Returns an iterator over the key-value pairs of the map.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            inner: self.buckets.iter(),
        }
    }
}

impl<K: Hash + Eq, V, S: BuildHasher> HashMap<K, V, S> {
    /// Returns the index of the bucket holding `key`, or `None` if absent.
    fn find(&self, key: &K) -> Option<usize> {
        if self.buckets.is_empty() {
            return None;
        }
        let mask = self.buckets.len() - 1;
        let mut idx = self.hash(key) & mask;
        loop {
            match &self.buckets[idx] {
                Some((k, _)) if k == key => return Some(idx),
                Some(_) => idx = (idx + 1) & mask,
                None => return None,
            }
        }
    }

    fn hash(&self, key: &K) -> usize {
        let mut hasher = self.hasher.build_hasher();
        key.hash(&mut hasher);
        hasher.finish() as usize
    }

    /// Returns a reference to the value corresponding to the key.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.find(key).map(|idx| &self.buckets[idx].as_ref().unwrap().1)
    }

    /// Returns `true` if the map contains the given key.
    pub fn contains_key(&self, key: &K) -> bool {
        self.find(key).is_some()
    }

    /// Inserts a key-value pair into the map, returning the previous value.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        // Keep the load factor below 3/4, starting with a small table.
        if self.buckets.is_empty() {
            self.resize(8);
        } else if (self.len + 1) * 4 > self.buckets.len() * 3 {
            self.resize(self.buckets.len() * 2);
        }

        let mask = self.buckets.len() - 1;
        let mut idx = self.hash(&key) & mask;
        loop {
            match &mut self.buckets[idx] {
                Some((k, v)) if *k == key => {
                    return Some(core::mem::replace(v, value));
                }
                Some(_) => idx = (idx + 1) & mask,
                slot @ None => {
                    *slot = Some((key, value));
                    self.len += 1;
                    return None;
                }
            }
        }
    }

    /// Removes a key from the map, returning the value if it was present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let idx = self.find(key)?;
        let mask = self.buckets.len() - 1;
        let removed = self.buckets[idx].take().map(|(_, v)| v);
        self.len -= 1;

        // Shift the rest of the probe cluster back so that no tombstone is
        // needed and lookups can stop at the first empty bucket.
        let mut next = (idx + 1) & mask;
        while let Some((k, v)) = self.buckets[next].take() {
            let mut target = self.hash(&k) & mask;
            while self.buckets[target].is_some() {
                target = (target + 1) & mask;
            }
            self.buckets[target] = Some((k, v));
            next = (next + 1) & mask;
        }
        removed
    }

    /// Rebuilds the table with at least `capacity` buckets (rounded up to a
    /// power of two) and re-inserts every entry.
    fn resize(&mut self, capacity: usize) {
        let capacity = capacity.max(8).next_power_of_two();
        let old = core::mem::take(&mut self.buckets);
        self.buckets = core::iter::repeat_with(|| None).take(capacity).collect();
        self.len = 0;
        for (k, v) in old.into_iter().flatten() {
            self.insert(k, v);
        }
    }
}

impl<'a, K: Hash + Eq, V, S: BuildHasher> IntoIterator for &'a HashMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// An iterator over the entries of a [`HashMap`].
pub struct Iter<'a, K, V> {
    inner: core::slice::Iter<'a, Option<(K, V)>>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        for slot in self.inner.by_ref() {
            if let Some((k, v)) = slot {
                return Some((k, v));
            }
        }
        None
    }
}
