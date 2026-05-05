use rustc_hash::FxHashMap;
use std::collections::VecDeque;

pub trait EvictionPolicy: Send {
    fn access(&mut self, key: u64, value_size: u64, t: u64) -> bool;
}

#[inline(always)]
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

// --- Allkeys-Random ---

pub struct AllkeysRandom {
    capacity_bytes: u64,
    resident_bytes: u64,
    keys: Vec<u64>,
    value_size: Vec<u64>,
    key_to_idx: FxHashMap<u64, usize>,
    rng: u64,
}

impl AllkeysRandom {
    pub fn new(capacity_bytes: u64, seed: u64) -> Self {
        Self {
            capacity_bytes,
            resident_bytes: 0,
            keys: Vec::new(),
            value_size: Vec::new(),
            key_to_idx: FxHashMap::default(),
            rng: seed | 1,
        }
    }

    #[inline(always)]
    fn evict_one(&mut self) {
        let n = self.keys.len();
        if n == 0 { return; }
        let idx = xorshift(&mut self.rng) as usize % n;
        self.resident_bytes -= self.value_size[idx];
        let victim_key = self.keys[idx];
        let last = n - 1;
        if idx != last {
            self.keys[idx] = self.keys[last];
            self.value_size[idx] = self.value_size[last];
            *self.key_to_idx.get_mut(&self.keys[idx]).unwrap() = idx;
        }
        self.keys.pop();
        self.value_size.pop();
        self.key_to_idx.remove(&victim_key);
    }
}

impl EvictionPolicy for AllkeysRandom {
    #[inline]
    fn access(&mut self, key: u64, value_size: u64, _t: u64) -> bool {
        if self.key_to_idx.contains_key(&key) {
            return true;
        }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 {
            return false;
        }
        let idx = self.keys.len();
        self.keys.push(key);
        self.value_size.push(value_size);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            self.evict_one();
        }
        false
    }
}

// --- Allkeys-LRU ---

pub struct AllkeysLru {
    capacity_bytes: u64,
    resident_bytes: u64,
    samples: usize,
    keys: Vec<u64>,
    last_access: Vec<u64>,
    value_size: Vec<u64>,
    key_to_idx: FxHashMap<u64, usize>,
    rng: u64,
}

impl AllkeysLru {
    pub fn new(capacity_bytes: u64, samples: usize, seed: u64) -> Self {
        Self {
            capacity_bytes,
            resident_bytes: 0,
            samples,
            keys: Vec::new(),
            last_access: Vec::new(),
            value_size: Vec::new(),
            key_to_idx: FxHashMap::default(),
            rng: seed | 1,
        }
    }

    #[inline(always)]
    fn evict_one(&mut self, t: u64) {
        let n = self.keys.len();
        if n == 0 { return; }
        let ns = self.samples.min(n);
        let mut best_idx = 0usize;
        let mut best_idle = 0u64;
        for _ in 0..ns {
            let i = xorshift(&mut self.rng) as usize % n;
            let idle = t - self.last_access[i];
            if idle > best_idle {
                best_idle = idle;
                best_idx = i;
            }
        }
        self.resident_bytes -= self.value_size[best_idx];
        let victim_key = self.keys[best_idx];
        let last = n - 1;
        if best_idx != last {
            self.keys[best_idx] = self.keys[last];
            self.last_access[best_idx] = self.last_access[last];
            self.value_size[best_idx] = self.value_size[last];
            *self.key_to_idx.get_mut(&self.keys[best_idx]).unwrap() = best_idx;
        }
        self.keys.pop();
        self.last_access.pop();
        self.value_size.pop();
        self.key_to_idx.remove(&victim_key);
    }
}

impl EvictionPolicy for AllkeysLru {
    #[inline]
    fn access(&mut self, key: u64, value_size: u64, t: u64) -> bool {
        if let Some(&idx) = self.key_to_idx.get(&key) {
            self.last_access[idx] = t;
            return true;
        }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 {
            return false;
        }
        let idx = self.keys.len();
        self.keys.push(key);
        self.last_access.push(t);
        self.value_size.push(value_size);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            self.evict_one(t);
        }
        false
    }
}

// --- Allkeys-LFU ---

pub struct AllkeysLfu {
    capacity_bytes: u64,
    resident_bytes: u64,
    samples: usize,
    log_factor: u64,
    decay_time: u64,
    time_scale: u64,
    keys: Vec<u64>,
    value_size: Vec<u64>,
    counter: Vec<u8>,
    last_decay_time: Vec<u64>,
    key_to_idx: FxHashMap<u64, usize>,
    rng: u64,
}

impl AllkeysLfu {
    pub fn new(
        capacity_bytes: u64,
        samples: usize,
        log_factor: u64,
        decay_time: u64,
        time_scale: u64,
        seed: u64,
    ) -> Self {
        Self {
            capacity_bytes,
            resident_bytes: 0,
            samples,
            log_factor,
            decay_time,
            time_scale,
            keys: Vec::new(),
            value_size: Vec::new(),
            counter: Vec::new(),
            last_decay_time: Vec::new(),
            key_to_idx: FxHashMap::default(),
            rng: seed | 1,
        }
    }

    #[inline(always)]
    fn minutes(&self, t: u64) -> u64 {
        t / self.time_scale
    }

    #[inline(always)]
    fn decayed_counter(&self, idx: usize, minutes_now: u64) -> u8 {
        if self.decay_time == 0 {
            return self.counter[idx];
        }
        let elapsed = minutes_now - self.last_decay_time[idx];
        let periods = (elapsed / self.decay_time) as u8;
        self.counter[idx].saturating_sub(periods)
    }

    #[inline(always)]
    fn evict_one(&mut self, t: u64) {
        let n = self.keys.len();
        if n == 0 { return; }
        let ns = self.samples.min(n);
        let minutes_now = self.minutes(t);
        let mut best_idx = 0usize;
        let mut best_score = 0u64;
        for _ in 0..ns {
            let i = xorshift(&mut self.rng) as usize % n;
            let decayed = self.decayed_counter(i, minutes_now);
            let score = 255 - decayed as u64;
            if score > best_score {
                best_score = score;
                best_idx = i;
            }
        }
        self.resident_bytes -= self.value_size[best_idx];
        let victim_key = self.keys[best_idx];
        let last = n - 1;
        if best_idx != last {
            self.keys[best_idx] = self.keys[last];
            self.value_size[best_idx] = self.value_size[last];
            self.counter[best_idx] = self.counter[last];
            self.last_decay_time[best_idx] = self.last_decay_time[last];
            *self.key_to_idx.get_mut(&self.keys[best_idx]).unwrap() = best_idx;
        }
        self.keys.pop();
        self.value_size.pop();
        self.counter.pop();
        self.last_decay_time.pop();
        self.key_to_idx.remove(&victim_key);
    }
}

impl EvictionPolicy for AllkeysLfu {
    #[inline]
    fn access(&mut self, key: u64, value_size: u64, t: u64) -> bool {
        let minutes_now = self.minutes(t);
        if let Some(&idx) = self.key_to_idx.get(&key) {
            let elapsed = minutes_now - self.last_decay_time[idx];
            if self.decay_time > 0 {
                let periods = (elapsed / self.decay_time) as u8;
                self.counter[idx] = self.counter[idx].saturating_sub(periods);
            }
            self.last_decay_time[idx] = minutes_now;
            let c = self.counter[idx];
            if c < 255 {
                let base = if c > 5 { (c - 5) as u64 } else { 0 };
                let p_denom = base * self.log_factor + 1;
                if xorshift(&mut self.rng) % p_denom == 0 {
                    self.counter[idx] = c + 1;
                }
            }
            return true;
        }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 {
            return false;
        }
        let idx = self.keys.len();
        self.keys.push(key);
        self.value_size.push(value_size);
        self.counter.push(5);
        self.last_decay_time.push(minutes_now);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            self.evict_one(t);
        }
        false
    }
}

// --- FIFO ---

pub struct Fifo {
    capacity_bytes: u64,
    resident_bytes: u64,
    queue: VecDeque<(u64, u64)>,
    resident: FxHashMap<u64, u64>,
}

impl Fifo {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            resident_bytes: 0,
            queue: VecDeque::new(),
            resident: FxHashMap::default(),
        }
    }
}

impl EvictionPolicy for Fifo {
    #[inline]
    fn access(&mut self, key: u64, value_size: u64, _t: u64) -> bool {
        if self.resident.contains_key(&key) {
            return true;
        }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 {
            return false;
        }
        self.resident.insert(key, value_size);
        self.queue.push_back((key, value_size));
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            if let Some((k, sz)) = self.queue.pop_front() {
                if self.resident.remove(&k).is_some() {
                    self.resident_bytes -= sz;
                }
            }
        }
        false
    }
}

// --- S3-FIFO (per libCacheSim reference) ---
// Small FIFO (10%) + Main FIFO (90%) with 2-bit CLOCK reinsertion + Ghost (90%).
// move_to_main_threshold = 1 (freq >= 1 to promote from small to main).
// Main reinsertion: freq = min(freq, 3) - 1; evict when freq == 0.
// Eviction: if main > main_target || small empty → evict main, else evict small.

pub struct S3Fifo {
    capacity: u64,
    small_target: u64,
    main_target: u64,
    small_bytes: u64,
    small_queue: VecDeque<(u64, u64)>,   // (key, size)
    small_freq: FxHashMap<u64, u8>,      // key -> freq (2-bit clock)
    main_bytes: u64,
    main_queue: VecDeque<(u64, u64)>,    // (key, size)
    main_freq: FxHashMap<u64, u8>,       // key -> freq (2-bit clock)
    ghost: FxHashMap<u64, ()>,
    ghost_queue: VecDeque<u64>,
    ghost_target: usize,
}

impl S3Fifo {
    pub fn new(capacity_bytes: u64) -> Self {
        Self::with_params(capacity_bytes, 0.10, 0.90, 1)
    }

    pub fn with_params(capacity_bytes: u64, small_ratio: f64, ghost_ratio: f64, _threshold: u8) -> Self {
        let small_target = (capacity_bytes as f64 * small_ratio).max(1.0) as u64;
        let main_target = capacity_bytes.saturating_sub(small_target);
        let ghost_target = if capacity_bytes > 0 {
            // Estimate ghost size in objects using average object size heuristic
            // Will be refined lazily on first access
            (capacity_bytes as f64 * ghost_ratio / 17000.0).max(100.0) as usize
        } else { 100 };
        Self {
            capacity: capacity_bytes,
            small_target,
            main_target,
            small_bytes: 0,
            small_queue: VecDeque::new(), small_freq: FxHashMap::default(),
            main_bytes: 0,
            main_queue: VecDeque::new(), main_freq: FxHashMap::default(),
            ghost: FxHashMap::default(),
            ghost_queue: VecDeque::new(),
            ghost_target,
        }
    }

    #[inline]
    fn total_bytes(&self) -> u64 { self.small_bytes + self.main_bytes }

    /// Evict one item from main. Returns true if an item was actually freed.
    fn evict_main(&mut self) -> bool {
        let mut budget = self.main_queue.len() * 3 + 1;
        while budget > 0 {
            budget -= 1;
            if let Some((k, sz)) = self.main_queue.pop_front() {
                let freq = match self.main_freq.remove(&k) {
                    Some(f) => f,
                    None => continue, // stale
                };
                if freq >= 1 {
                    // Reinsertion with CLOCK: freq = min(freq, 3) - 1
                    let new_freq = freq.min(3) - 1;
                    self.main_queue.push_back((k, sz));
                    self.main_freq.insert(k, new_freq);
                } else {
                    // Evict
                    self.main_bytes -= sz;
                    return true;
                }
            } else {
                return false;
            }
        }
        false
    }

    /// Evict from small: promote freq>=1 to main, demote freq==0 to ghost.
    /// Returns true if an item was actually freed (demoted).
    fn evict_small(&mut self) -> bool {
        while let Some((k, sz)) = self.small_queue.pop_front() {
            let freq = match self.small_freq.remove(&k) {
                Some(f) => f,
                None => continue,
            };
            self.small_bytes -= sz;
            if freq >= 1 {
                // Promote to main
                self.main_queue.push_back((k, sz));
                self.main_freq.insert(k, 0);
                self.main_bytes += sz;
                // Not freed — caller must continue
            } else {
                // Demote to ghost
                self.add_ghost(k);
                return true;
            }
        }
        false
    }

    fn evict(&mut self) {
        while self.total_bytes() > self.capacity {
            // Reference logic: evict main if main > target or small is empty
            if self.main_bytes > self.main_target || self.small_queue.is_empty() {
                if !self.evict_main() { break; }
            } else {
                if !self.evict_small() {
                    // Small only promoted, now main is over → evict main
                    if !self.evict_main() { break; }
                }
            }
        }
    }

    fn add_ghost(&mut self, key: u64) {
        self.ghost_queue.push_back(key);
        self.ghost.insert(key, ());
        let cap = self.ghost_target.max(100);
        while self.ghost_queue.len() > cap {
            if let Some(old) = self.ghost_queue.pop_front() {
                self.ghost.remove(&old);
            }
        }
    }
}

impl EvictionPolicy for S3Fifo {
    #[inline]
    fn access(&mut self, key: u64, value_size: u64, _t: u64) -> bool {
        // Hit in main — bump freq (2-bit clock, max 3)
        if let Some(f) = self.main_freq.get_mut(&key) {
            if *f < 3 { *f += 1; }
            return true;
        }
        // Hit in small — bump freq
        if let Some(f) = self.small_freq.get_mut(&key) {
            if *f < 3 { *f += 1; }
            return true;
        }
        // Miss
        if value_size > self.capacity || self.capacity == 0 {
            return false;
        }
        if self.ghost.remove(&key).is_some() {
            // Ghost hit → insert to main
            self.main_queue.push_back((key, value_size));
            self.main_freq.insert(key, 0);
            self.main_bytes += value_size;
        } else {
            // Cold miss → insert to small
            if value_size > self.small_target {
                return false; // too big for small FIFO
            }
            self.small_queue.push_back((key, value_size));
            self.small_freq.insert(key, 0);
            self.small_bytes += value_size;
        }
        self.evict();
        // Lazily set ghost target (90% of capacity in object count)
        if self.ghost_target == 0 && value_size > 0 {
            self.ghost_target = (self.main_target / value_size).max(100) as usize;
        }
        false
    }
}
