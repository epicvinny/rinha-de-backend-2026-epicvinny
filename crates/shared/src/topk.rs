// Top-5 nearest neighbors with tie-breaking by original reference ID.
// Packed as u64: (dist_sq as u32) << 32 | (original_id as u32) for lex compare.
#[derive(Clone, Copy)]
pub struct TopKEntry {
    key: u64,
    label: u8,
}

pub struct TopK {
    entries: [TopKEntry; 5],
    count: usize,
}

impl TopK {
    pub fn new() -> Self {
        TopK {
            entries: [TopKEntry {
                key: 0x7FFFFFFF_FFFFFFFFu64,
                label: 0,
            }; 5],
            count: 0,
        }
    }

    #[inline]
    pub fn make_key(dist_sq: i32, original_id: u32) -> u64 {
        ((dist_sq as u32 as u64) << 32) | (original_id as u64)
    }

    #[inline]
    pub fn insert(&mut self, dist_sq: i32, original_id: u32) {
        self.insert_labeled(dist_sq, original_id, 0);
    }

    #[inline]
    pub fn insert_labeled(&mut self, dist_sq: i32, original_id: u32, label: u8) {
        let key = Self::make_key(dist_sq, original_id);
        if key < self.entries[4].key {
            let mut i = 4;
            while i > 0 && self.entries[i - 1].key > key {
                self.entries[i] = self.entries[i - 1];
                i -= 1;
            }
            self.entries[i] = TopKEntry { key, label };
            if self.count < 5 {
                self.count += 1;
            }
        }
    }

    #[inline]
    pub fn worst_dist_sq(&self) -> i32 {
        (self.entries[4].key >> 32) as i32
    }

    #[inline]
    pub fn worst_original_id(&self) -> u32 {
        self.entries[4].key as u32
    }

    #[inline]
    pub fn worst_key(&self) -> u64 {
        self.entries[4].key
    }

    pub fn is_full(&self) -> bool {
        self.count == 5
    }

    pub fn results(&self) -> &[TopKEntry] {
        &self.entries[..self.count]
    }

    pub fn ids(&self) -> [u32; 5] {
        let mut ids = [u32::MAX; 5];
        for (i, &e) in self.entries[..self.count].iter().enumerate() {
            ids[i] = e.key as u32;
        }
        ids
    }

    pub fn distances(&self) -> [i32; 5] {
        let mut distances = [i32::MAX; 5];
        for (i, &e) in self.entries[..self.count].iter().enumerate() {
            distances[i] = (e.key >> 32) as i32;
        }
        distances
    }

    pub fn fraud_count(&self) -> usize {
        self.entries[..self.count]
            .iter()
            .filter(|entry| entry.label == 1)
            .count()
    }
}

impl Default for TopK {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topk_basic() {
        let mut tk = TopK::new();
        tk.insert(100, 5);
        tk.insert(50, 3);
        tk.insert(200, 1);
        tk.insert(50, 7);
        tk.insert(150, 2);
        // Full now. Sorted: (50,3), (50,7), (100,5), (150,2), (200,1)
        assert_eq!(tk.is_full(), true);
        let r = tk.results();
        // Lower dist first; on tie, lower id first
        let d0 = (r[0].key >> 32) as i32;
        let id0 = r[0].key as u32;
        assert_eq!(d0, 50);
        assert_eq!(id0, 3);
        let d1 = (r[1].key >> 32) as i32;
        let id1 = r[1].key as u32;
        assert_eq!(d1, 50);
        assert_eq!(id1, 7);
    }

    #[test]
    fn topk_eviction() {
        let mut tk = TopK::new();
        for i in 0..5 {
            tk.insert(1000, i as u32);
        }
        // Try to insert something worse — should be rejected
        tk.insert(2000, 99);
        let r = tk.results();
        assert!((r[4].key >> 32) as i32 <= 1000);
        // Insert something better
        tk.insert(1, 42);
        let r = tk.results();
        assert_eq!((r[0].key >> 32) as i32, 1);
        assert_eq!(r[0].key as u32, 42);
    }

    #[test]
    fn labels_are_kept_with_entries() {
        let mut tk = TopK::new();
        tk.insert_labeled(10, 1, 1);
        tk.insert_labeled(20, 2, 0);
        tk.insert_labeled(30, 3, 1);

        assert_eq!(tk.fraud_count(), 2);
    }
}
