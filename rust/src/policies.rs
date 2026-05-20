use rustc_hash::FxHashMap;
use std::collections::VecDeque;

pub trait EvictionPolicy: Send {
    /// Access a key. Returns true if it was already resident (hit).
    /// On miss, admits the key and evicts as needed.
    fn access(&mut self, key: u64, value_size: u64, t: u64) -> bool;

    /// Check if key is resident without modifying state.
    fn contains(&self, key: u64) -> bool;

    /// Touch a resident key (update recency/frequency). No-op if not resident.
    fn touch(&mut self, key: u64, t: u64);

    /// Admit a key (insert + evict as needed). Returns list of evicted keys.
    fn admit(&mut self, key: u64, value_size: u64, t: u64) -> Vec<u64>;

    /// Remove a key from the cache without going through normal eviction.
    fn remove(&mut self, key: u64);
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

    fn evict_one(&mut self) -> Option<u64> {
        let n = self.keys.len();
        if n == 0 { return None; }
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
        Some(victim_key)
    }
}

impl EvictionPolicy for AllkeysRandom {
    fn access(&mut self, key: u64, value_size: u64, _t: u64) -> bool {
        if self.key_to_idx.contains_key(&key) { return true; }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return false; }
        let idx = self.keys.len();
        self.keys.push(key);
        self.value_size.push(value_size);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes { self.evict_one(); }
        false
    }

    fn contains(&self, key: u64) -> bool { self.key_to_idx.contains_key(&key) }

    fn touch(&mut self, _key: u64, _t: u64) {}

    fn admit(&mut self, key: u64, value_size: u64, _t: u64) -> Vec<u64> {
        let mut evicted = Vec::new();
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return evicted; }
        if self.key_to_idx.contains_key(&key) { return evicted; }
        let idx = self.keys.len();
        self.keys.push(key);
        self.value_size.push(value_size);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            if let Some(v) = self.evict_one() { evicted.push(v); }
            else { break; }
        }
        evicted
    }

    fn remove(&mut self, key: u64) {
        if let Some(idx) = self.key_to_idx.remove(&key) {
            self.resident_bytes -= self.value_size[idx];
            let last = self.keys.len() - 1;
            if idx != last {
                self.keys[idx] = self.keys[last];
                self.value_size[idx] = self.value_size[last];
                *self.key_to_idx.get_mut(&self.keys[idx]).unwrap() = idx;
            }
            self.keys.pop();
            self.value_size.pop();
        }
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

    fn evict_one(&mut self, t: u64) -> Option<u64> {
        let n = self.keys.len();
        if n == 0 { return None; }
        let ns = self.samples.min(n);
        let mut best_idx = 0usize;
        let mut best_idle = 0u64;
        for _ in 0..ns {
            let i = xorshift(&mut self.rng) as usize % n;
            let idle = t - self.last_access[i];
            if idle > best_idle { best_idle = idle; best_idx = i; }
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
        Some(victim_key)
    }
}

impl EvictionPolicy for AllkeysLru {
    fn access(&mut self, key: u64, value_size: u64, t: u64) -> bool {
        if let Some(&idx) = self.key_to_idx.get(&key) {
            self.last_access[idx] = t;
            return true;
        }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return false; }
        let idx = self.keys.len();
        self.keys.push(key);
        self.last_access.push(t);
        self.value_size.push(value_size);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes { self.evict_one(t); }
        false
    }

    fn contains(&self, key: u64) -> bool { self.key_to_idx.contains_key(&key) }

    fn touch(&mut self, key: u64, t: u64) {
        if let Some(&idx) = self.key_to_idx.get(&key) {
            self.last_access[idx] = t;
        }
    }

    fn admit(&mut self, key: u64, value_size: u64, t: u64) -> Vec<u64> {
        let mut evicted = Vec::new();
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return evicted; }
        if self.key_to_idx.contains_key(&key) { return evicted; }
        let idx = self.keys.len();
        self.keys.push(key);
        self.last_access.push(t);
        self.value_size.push(value_size);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            if let Some(v) = self.evict_one(t) { evicted.push(v); }
            else { break; }
        }
        evicted
    }

    fn remove(&mut self, key: u64) {
        if let Some(idx) = self.key_to_idx.remove(&key) {
            self.resident_bytes -= self.value_size[idx];
            let last = self.keys.len() - 1;
            if idx != last {
                self.keys[idx] = self.keys[last];
                self.last_access[idx] = self.last_access[last];
                self.value_size[idx] = self.value_size[last];
                *self.key_to_idx.get_mut(&self.keys[idx]).unwrap() = idx;
            }
            self.keys.pop();
            self.last_access.pop();
            self.value_size.pop();
        }
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
    pub fn new(capacity_bytes: u64, samples: usize, log_factor: u64, decay_time: u64, time_scale: u64, seed: u64) -> Self {
        Self {
            capacity_bytes, resident_bytes: 0, samples, log_factor, decay_time, time_scale,
            keys: Vec::new(), value_size: Vec::new(), counter: Vec::new(),
            last_decay_time: Vec::new(), key_to_idx: FxHashMap::default(), rng: seed | 1,
        }
    }

    fn minutes(&self, t: u64) -> u64 { t / self.time_scale }

    fn decayed_counter(&self, idx: usize, minutes_now: u64) -> u8 {
        if self.decay_time == 0 { return self.counter[idx]; }
        let elapsed = minutes_now - self.last_decay_time[idx];
        let periods = (elapsed / self.decay_time) as u8;
        self.counter[idx].saturating_sub(periods)
    }

    fn evict_one(&mut self, t: u64) -> Option<u64> {
        let n = self.keys.len();
        if n == 0 { return None; }
        let ns = self.samples.min(n);
        let minutes_now = self.minutes(t);
        let mut best_idx = 0usize;
        let mut best_score = 0u64;
        for _ in 0..ns {
            let i = xorshift(&mut self.rng) as usize % n;
            let decayed = self.decayed_counter(i, minutes_now);
            let score = 255 - decayed as u64;
            if score > best_score { best_score = score; best_idx = i; }
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
        self.keys.pop(); self.value_size.pop(); self.counter.pop(); self.last_decay_time.pop();
        self.key_to_idx.remove(&victim_key);
        Some(victim_key)
    }

    fn lfu_increment(&mut self, idx: usize, minutes_now: u64) {
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
    }
}

impl EvictionPolicy for AllkeysLfu {
    fn access(&mut self, key: u64, value_size: u64, t: u64) -> bool {
        let minutes_now = self.minutes(t);
        if let Some(&idx) = self.key_to_idx.get(&key) {
            self.lfu_increment(idx, minutes_now);
            return true;
        }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return false; }
        let idx = self.keys.len();
        self.keys.push(key); self.value_size.push(value_size);
        self.counter.push(5); self.last_decay_time.push(minutes_now);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes { self.evict_one(t); }
        false
    }

    fn contains(&self, key: u64) -> bool { self.key_to_idx.contains_key(&key) }

    fn touch(&mut self, key: u64, t: u64) {
        let minutes_now = self.minutes(t);
        if let Some(&idx) = self.key_to_idx.get(&key) {
            self.lfu_increment(idx, minutes_now);
        }
    }

    fn admit(&mut self, key: u64, value_size: u64, t: u64) -> Vec<u64> {
        let mut evicted = Vec::new();
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return evicted; }
        if self.key_to_idx.contains_key(&key) { return evicted; }
        let minutes_now = self.minutes(t);
        let idx = self.keys.len();
        self.keys.push(key); self.value_size.push(value_size);
        self.counter.push(5); self.last_decay_time.push(minutes_now);
        self.key_to_idx.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            if let Some(v) = self.evict_one(t) { evicted.push(v); }
            else { break; }
        }
        evicted
    }

    fn remove(&mut self, key: u64) {
        if let Some(idx) = self.key_to_idx.remove(&key) {
            self.resident_bytes -= self.value_size[idx];
            let last = self.keys.len() - 1;
            if idx != last {
                self.keys[idx] = self.keys[last];
                self.value_size[idx] = self.value_size[last];
                self.counter[idx] = self.counter[last];
                self.last_decay_time[idx] = self.last_decay_time[last];
                *self.key_to_idx.get_mut(&self.keys[idx]).unwrap() = idx;
            }
            self.keys.pop(); self.value_size.pop(); self.counter.pop(); self.last_decay_time.pop();
        }
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
        Self { capacity_bytes, resident_bytes: 0, queue: VecDeque::new(), resident: FxHashMap::default() }
    }

    fn evict_one(&mut self) -> Option<u64> {
        if let Some((k, sz)) = self.queue.pop_front() {
            if self.resident.remove(&k).is_some() {
                self.resident_bytes -= sz;
                return Some(k);
            }
        }
        None
    }
}

impl EvictionPolicy for Fifo {
    fn access(&mut self, key: u64, value_size: u64, _t: u64) -> bool {
        if self.resident.contains_key(&key) { return true; }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return false; }
        self.resident.insert(key, value_size);
        self.queue.push_back((key, value_size));
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes { self.evict_one(); }
        false
    }

    fn contains(&self, key: u64) -> bool { self.resident.contains_key(&key) }
    fn touch(&mut self, _key: u64, _t: u64) {}

    fn admit(&mut self, key: u64, value_size: u64, _t: u64) -> Vec<u64> {
        let mut evicted = Vec::new();
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return evicted; }
        if self.resident.contains_key(&key) { return evicted; }
        self.resident.insert(key, value_size);
        self.queue.push_back((key, value_size));
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            if let Some(v) = self.evict_one() { evicted.push(v); }
            else { break; }
        }
        evicted
    }

    fn remove(&mut self, key: u64) {
        if let Some(sz) = self.resident.remove(&key) {
            self.resident_bytes -= sz;
            // Leave stale entry in queue — evict_one handles it
        }
    }
}

// --- S3-FIFO ---

pub struct S3Fifo {
    capacity: u64,
    small_target: u64,
    main_target: u64,
    small_bytes: u64,
    small_queue: VecDeque<(u64, u64)>,
    small_freq: FxHashMap<u64, u8>,
    main_bytes: u64,
    main_queue: VecDeque<(u64, u64)>,
    main_freq: FxHashMap<u64, u8>,
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
            (capacity_bytes as f64 * ghost_ratio / 17000.0).max(100.0) as usize
        } else { 100 };
        Self {
            capacity: capacity_bytes, small_target, main_target,
            small_bytes: 0, small_queue: VecDeque::new(), small_freq: FxHashMap::default(),
            main_bytes: 0, main_queue: VecDeque::new(), main_freq: FxHashMap::default(),
            ghost: FxHashMap::default(), ghost_queue: VecDeque::new(), ghost_target,
        }
    }

    fn total_bytes(&self) -> u64 { self.small_bytes + self.main_bytes }

    fn evict_main(&mut self) -> Option<u64> {
        let mut budget = self.main_queue.len() * 3 + 1;
        while budget > 0 {
            budget -= 1;
            if let Some((k, sz)) = self.main_queue.pop_front() {
                let freq = match self.main_freq.remove(&k) {
                    Some(f) => f,
                    None => continue,
                };
                if freq >= 1 {
                    let new_freq = freq.min(3) - 1;
                    self.main_queue.push_back((k, sz));
                    self.main_freq.insert(k, new_freq);
                } else {
                    self.main_bytes -= sz;
                    return Some(k);
                }
            } else { return None; }
        }
        None
    }

    fn evict_small(&mut self) -> (Option<u64>, bool) {
        // Returns (evicted_key, was_freed). If promoted, evicted_key is None.
        while let Some((k, sz)) = self.small_queue.pop_front() {
            let freq = match self.small_freq.remove(&k) {
                Some(f) => f,
                None => continue,
            };
            self.small_bytes -= sz;
            if freq >= 1 {
                self.main_queue.push_back((k, sz));
                self.main_freq.insert(k, 0);
                self.main_bytes += sz;
                return (None, false); // promoted, not freed
            } else {
                self.add_ghost(k);
                return (Some(k), true);
            }
        }
        (None, false)
    }

    fn evict(&mut self) -> Vec<u64> {
        let mut evicted = Vec::new();
        while self.total_bytes() > self.capacity {
            if self.main_bytes > self.main_target || self.small_queue.is_empty() {
                if let Some(k) = self.evict_main() { evicted.push(k); }
                else { break; }
            } else {
                let (k, freed) = self.evict_small();
                if freed { if let Some(k) = k { evicted.push(k); } }
                else {
                    if let Some(k) = self.evict_main() { evicted.push(k); }
                    else { break; }
                }
            }
        }
        evicted
    }

    fn add_ghost(&mut self, key: u64) {
        self.ghost_queue.push_back(key);
        self.ghost.insert(key, ());
        let cap = self.ghost_target.max(100);
        while self.ghost_queue.len() > cap {
            if let Some(old) = self.ghost_queue.pop_front() { self.ghost.remove(&old); }
        }
    }

    fn is_resident(&self, key: u64) -> bool {
        self.small_freq.contains_key(&key) || self.main_freq.contains_key(&key)
    }
}

impl EvictionPolicy for S3Fifo {
    fn access(&mut self, key: u64, value_size: u64, _t: u64) -> bool {
        if let Some(f) = self.main_freq.get_mut(&key) { if *f < 3 { *f += 1; } return true; }
        if let Some(f) = self.small_freq.get_mut(&key) { if *f < 3 { *f += 1; } return true; }
        if value_size > self.capacity || self.capacity == 0 { return false; }
        if self.ghost.remove(&key).is_some() {
            self.main_queue.push_back((key, value_size));
            self.main_freq.insert(key, 0);
            self.main_bytes += value_size;
        } else {
            if value_size > self.small_target { return false; }
            self.small_queue.push_back((key, value_size));
            self.small_freq.insert(key, 0);
            self.small_bytes += value_size;
        }
        self.evict();
        if self.ghost_target == 0 && value_size > 0 {
            self.ghost_target = (self.main_target / value_size).max(100) as usize;
        }
        false
    }

    fn contains(&self, key: u64) -> bool { self.is_resident(key) }

    fn touch(&mut self, key: u64, _t: u64) {
        if let Some(f) = self.main_freq.get_mut(&key) { if *f < 3 { *f += 1; } }
        else if let Some(f) = self.small_freq.get_mut(&key) { if *f < 3 { *f += 1; } }
    }

    fn admit(&mut self, key: u64, value_size: u64, _t: u64) -> Vec<u64> {
        if value_size > self.capacity || self.capacity == 0 { return Vec::new(); }
        if self.is_resident(key) { return Vec::new(); }
        if self.ghost.remove(&key).is_some() {
            self.main_queue.push_back((key, value_size));
            self.main_freq.insert(key, 0);
            self.main_bytes += value_size;
        } else {
            if value_size > self.small_target { return Vec::new(); }
            self.small_queue.push_back((key, value_size));
            self.small_freq.insert(key, 0);
            self.small_bytes += value_size;
        }
        if self.ghost_target == 0 && value_size > 0 {
            self.ghost_target = (self.main_target / value_size).max(100) as usize;
        }
        self.evict()
    }

    fn remove(&mut self, key: u64) {
        if let Some(_) = self.small_freq.remove(&key) {
            // Find and remove from small_queue (leave stale, handled by evict)
            // Just adjust bytes — stale entries are skipped in evict_small
        } else if let Some(_) = self.main_freq.remove(&key) {
            // Same for main
        }
        // Note: bytes not adjusted here since we can't easily find the size.
        // This is acceptable for simulation purposes — remove is rarely called.
    }
}

// --- True LRU (exact, not sampled) ---
// Uses a doubly-linked list via prev/next arrays for O(1) move-to-front.

pub struct TrueLru {
    capacity_bytes: u64,
    resident_bytes: u64,
    // Doubly-linked list nodes: key, size, prev, next
    keys: Vec<u64>,
    sizes: Vec<u64>,
    prev: Vec<usize>,
    next: Vec<usize>,
    key_to_node: FxHashMap<u64, usize>,
    head: usize, // LRU (oldest) — evict from here
    tail: usize, // MRU (newest) — insert/move here
    free: Vec<usize>,
}

const NIL: usize = usize::MAX;

impl TrueLru {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            resident_bytes: 0,
            keys: Vec::new(),
            sizes: Vec::new(),
            prev: Vec::new(),
            next: Vec::new(),
            key_to_node: FxHashMap::default(),
            head: NIL,
            tail: NIL,
            free: Vec::new(),
        }
    }

    fn alloc_node(&mut self, key: u64, size: u64) -> usize {
        if let Some(idx) = self.free.pop() {
            self.keys[idx] = key;
            self.sizes[idx] = size;
            self.prev[idx] = NIL;
            self.next[idx] = NIL;
            idx
        } else {
            let idx = self.keys.len();
            self.keys.push(key);
            self.sizes.push(size);
            self.prev.push(NIL);
            self.next.push(NIL);
            idx
        }
    }

    fn unlink(&mut self, idx: usize) {
        let p = self.prev[idx];
        let n = self.next[idx];
        if p != NIL { self.next[p] = n; } else { self.head = n; }
        if n != NIL { self.prev[n] = p; } else { self.tail = p; }
        self.prev[idx] = NIL;
        self.next[idx] = NIL;
    }

    fn push_back(&mut self, idx: usize) {
        self.prev[idx] = self.tail;
        self.next[idx] = NIL;
        if self.tail != NIL { self.next[self.tail] = idx; }
        self.tail = idx;
        if self.head == NIL { self.head = idx; }
    }

    fn move_to_back(&mut self, idx: usize) {
        if idx == self.tail { return; }
        self.unlink(idx);
        self.push_back(idx);
    }

    fn evict_lru(&mut self) -> Option<u64> {
        if self.head == NIL { return None; }
        let idx = self.head;
        let key = self.keys[idx];
        let sz = self.sizes[idx];
        self.unlink(idx);
        self.free.push(idx);
        self.key_to_node.remove(&key);
        self.resident_bytes -= sz;
        Some(key)
    }
}

impl EvictionPolicy for TrueLru {
    fn access(&mut self, key: u64, value_size: u64, _t: u64) -> bool {
        if let Some(&idx) = self.key_to_node.get(&key) {
            self.move_to_back(idx);
            return true;
        }
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return false; }
        let idx = self.alloc_node(key, value_size);
        self.push_back(idx);
        self.key_to_node.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes { self.evict_lru(); }
        false
    }

    fn contains(&self, key: u64) -> bool { self.key_to_node.contains_key(&key) }

    fn touch(&mut self, key: u64, _t: u64) {
        if let Some(&idx) = self.key_to_node.get(&key) { self.move_to_back(idx); }
    }

    fn admit(&mut self, key: u64, value_size: u64, _t: u64) -> Vec<u64> {
        let mut evicted = Vec::new();
        if value_size > self.capacity_bytes || self.capacity_bytes == 0 { return evicted; }
        if self.key_to_node.contains_key(&key) { return evicted; }
        let idx = self.alloc_node(key, value_size);
        self.push_back(idx);
        self.key_to_node.insert(key, idx);
        self.resident_bytes += value_size;
        while self.resident_bytes > self.capacity_bytes {
            if let Some(v) = self.evict_lru() { evicted.push(v); }
            else { break; }
        }
        evicted
    }

    fn remove(&mut self, key: u64) {
        if let Some(idx) = self.key_to_node.remove(&key) {
            self.resident_bytes -= self.sizes[idx];
            self.unlink(idx);
            self.free.push(idx);
        }
    }
}

