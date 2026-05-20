use rustc_hash::FxHashSet;

pub trait PromotionPolicy: Send {
    /// Called on flash hit. Returns true if key should be promoted to DRAM.
    fn should_promote(&mut self, key: u64, t: u64) -> bool;
    fn label(&self) -> String;
}

/// Always promote on flash hit (baseline / naive).
pub struct AlwaysPromote;

impl PromotionPolicy for AlwaysPromote {
    #[inline]
    fn should_promote(&mut self, _key: u64, _t: u64) -> bool { true }
    fn label(&self) -> String { "always".to_string() }
}

/// Never promote — serve from flash only.
pub struct NeverPromote;

impl PromotionPolicy for NeverPromote {
    #[inline]
    fn should_promote(&mut self, _key: u64, _t: u64) -> bool { false }
    fn label(&self) -> String { "never".to_string() }
}

/// Promote on second hit within a window of `window` accesses.
/// Uses a rotating hash set (cheap Bloom-like filter).
pub struct SecondHit {
    window: usize,
    history: Vec<u64>,   // ring buffer of recent flash-hit keys
    pos: usize,
    set: FxHashSet<u64>,
}

impl SecondHit {
    pub fn new(window: usize) -> Self {
        Self {
            window,
            history: Vec::with_capacity(window),
            pos: 0,
            set: FxHashSet::default(),
        }
    }
}

impl PromotionPolicy for SecondHit {
    #[inline]
    fn should_promote(&mut self, key: u64, _t: u64) -> bool {
        if self.set.contains(&key) {
            // Second hit — promote and remove from history
            true
        } else {
            // First hit — record in history
            if self.history.len() < self.window {
                self.history.push(key);
                self.set.insert(key);
            } else {
                // Evict oldest from ring buffer
                let old = self.history[self.pos];
                self.set.remove(&old);
                self.history[self.pos] = key;
                self.set.insert(key);
                self.pos = (self.pos + 1) % self.window;
            }
            false
        }
    }

    fn label(&self) -> String { format!("second-hit-{}", self.window) }
}

/// Promote if reused within `threshold` accesses of last flash access.
/// Stores last-access time per key (in flash).
pub struct RecentReuse {
    threshold: u64,
    last_access: rustc_hash::FxHashMap<u64, u64>,
}

impl RecentReuse {
    pub fn new(threshold: u64) -> Self {
        Self { threshold, last_access: rustc_hash::FxHashMap::default() }
    }
}

impl PromotionPolicy for RecentReuse {
    #[inline]
    fn should_promote(&mut self, key: u64, t: u64) -> bool {
        if let Some(&prev) = self.last_access.get(&key) {
            let gap = t - prev;
            self.last_access.insert(key, t);
            gap <= self.threshold
        } else {
            self.last_access.insert(key, t);
            false
        }
    }

    fn label(&self) -> String { format!("reuse-within-{}", self.threshold) }
}
