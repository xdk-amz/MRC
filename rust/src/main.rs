// MRC Simulator — two-tier (DRAM + Flash) with promotion policies.
// Key invariant: no-key-spilling sweeps [total_key_bytes, total_bytes],
// policy_cap = cap_bytes - total_key_bytes. Never sweep below total_key_bytes.

use clap::Parser;
use csv::WriterBuilder;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::PathBuf;
use std::sync::Arc;

mod policies;
use policies::{AllkeysLfu, AllkeysLru, AllkeysRandom, Fifo, S3Fifo, TrueLru, EvictionPolicy};

mod promotion;
use promotion::{PromotionPolicy, AlwaysPromote, NeverPromote, SecondHit, RecentReuse};

mod admission;
use admission::{AdmissionPolicy, AdmitToDram, AdmitToFlash};

#[derive(Clone, Copy, PartialEq)]
enum SpillMode {
    KeySpilling,
    NoKeySpilling,
}

impl SpillMode {
    fn label(&self) -> &'static str {
        match self {
            SpillMode::KeySpilling => "key-spilling",
            SpillMode::NoKeySpilling => "no-key-spilling",
        }
    }
}

#[derive(Parser)]
#[command(name = "mrc-sim", about = "MRC simulator for Valkey eviction policies with promotion")]
struct Cli {
    /// Trace CSV files
    #[arg(required = true)]
    traces: Vec<PathBuf>,

    /// Eviction policies: allkeys-lru, allkeys-lfu, allkeys-random, fifo, s3-fifo
    #[arg(short, long, value_delimiter = ',', default_value = "allkeys-lru")]
    policy: Vec<String>,

    /// Promotion policies: always, never, second-hit-N, reuse-within-N
    #[arg(long, value_delimiter = ',', default_value = "always")]
    promotion: Vec<String>,

    /// Admission policies: admit-dram, admit-flash
    #[arg(long, value_delimiter = ',', default_value = "admit-dram")]
    admission: Vec<String>,

    /// Number of capacity points
    #[arg(short = 'n', long, default_value = "21")]
    cap_points: usize,

    /// Output directory
    #[arg(short, long, default_value = "out/valkey_mrc")]
    output: PathBuf,

    /// Spill mode: key-spilling, no-key-spilling, both
    #[arg(short, long, default_value = "both")]
    mode: String,

    /// Fixed per-key DRAM overhead in bytes
    #[arg(long, default_value = "0")]
    key_overhead: u64,

    /// Include first-touch accesses in miss ratio
    #[arg(long, default_value = "false")]
    include_first_touch: bool,

    /// Per-key DRAM overhead for storage index when key is spilled
    #[arg(long, default_value = "16")]
    key_spill_overhead: u64,
}

struct Trace {
    workload: String,
    keys: Vec<u64>,
    key_sizes: Vec<u64>,
    value_sizes: Vec<u64>,
    total_key_bytes: u64,
    total_value_bytes: u64,
    total_bytes: u64,
    n_unique: usize,
}

fn load_trace(path: &PathBuf, key_overhead: u64) -> Trace {
    let workload = path.file_stem().unwrap().to_string_lossy().to_string();

    let first_line = {
        let file = std::fs::File::open(path).expect("cannot open trace");
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        line.trim().to_string()
    };

    let skip_first = !first_line.contains(',')
        || first_line.starts_with("t,")
        || first_line.starts_with("ts_us,")
        || first_line.starts_with("workload,");

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .expect("cannot open trace");

    let mut keys = Vec::new();
    let mut key_sizes = Vec::new();
    let mut value_sizes = Vec::new();
    let mut key_map: FxHashMap<String, u64> = FxHashMap::default();
    let mut next_id: u64 = 0;

    for (i, result) in rdr.records().enumerate() {
        let record = result.expect("bad csv row");
        if i == 0 && skip_first { continue; }
        let ncols = record.len();

        if ncols >= 10 {
            // Valkey MONITOR TRACE: ts_us, seq, db_id, cmd, key(b64), access_type, key_exists, obj_type, key_bytes, value_bytes
            let key_str = &record[4];
            let key_id = *key_map.entry(key_str.to_string()).or_insert_with(|| {
                let id = next_id; next_id += 1; id
            });
            let kb: u64 = record[8].parse().unwrap_or(0);
            let vb: u64 = record[9].parse().unwrap_or(0);
            keys.push(key_id); key_sizes.push(kb); value_sizes.push(vb);
        } else if ncols >= 6 {
            // New synthetic: t, op, key, key_size, value_size, workload_label
            keys.push(record[2].parse::<u64>().expect("bad key"));
            key_sizes.push(record[3].parse::<u64>().expect("bad key_size"));
            value_sizes.push(record[4].parse::<u64>().expect("bad value_size"));
        } else if ncols >= 4 {
            // Twitter trace: timestamp, op, key, size
            let key_id = *key_map.entry(record[2].to_string()).or_insert_with(|| {
                let id = next_id; next_id += 1; id
            });
            let vb: u64 = record[3].parse().unwrap_or(0);
            keys.push(key_id); key_sizes.push(0); value_sizes.push(vb);
        } else {
            // Minimal: key, value_size
            keys.push(record[0].parse::<u64>().unwrap_or_else(|_| {
                *key_map.entry(record[0].to_string()).or_insert_with(|| {
                    let id = next_id; next_id += 1; id
                })
            }));
            key_sizes.push(0);
            value_sizes.push(record[1].parse::<u64>().expect("bad size"));
        }
    }

    if key_overhead > 0 {
        for ks in key_sizes.iter_mut() { *ks += key_overhead; }
    }

    let mut first_key_size: FxHashMap<u64, u64> = FxHashMap::default();
    let mut first_value_size: FxHashMap<u64, u64> = FxHashMap::default();
    for i in 0..keys.len() {
        first_key_size.entry(keys[i]).or_insert(key_sizes[i]);
        first_value_size.entry(keys[i]).or_insert(value_sizes[i]);
    }
    let total_key_bytes: u64 = first_key_size.values().sum();
    let total_value_bytes: u64 = first_value_size.values().sum();
    let total_bytes = total_key_bytes + total_value_bytes;
    let n_unique = first_key_size.len();

    Trace { workload, keys, key_sizes, value_sizes, total_key_bytes, total_value_bytes, total_bytes, n_unique }
}

struct TaskResult {
    workload: String,
    policy: String,
    promotion_policy: String,
    admission_policy: String,
    mode: SpillMode,
    cap_frac: f64,
    cap_bytes: u64,
    total_key_bytes: u64,
    total_value_bytes: u64,
    total_bytes: u64,
    n_unique: usize,
    total_accesses: usize,
    obj_miss_ratio: f64,
    byte_miss_ratio: f64,
    flash_hit_ratio: f64,
    promotions: u64,
    flash_hits: u64,
    evictions: u64,
    keys_ever_evicted: u64,
    keys_ever_promoted: u64,
    max_evictions_per_key: u64,
    max_promotions_per_key: u64,
    keys_evicted_gt1: u64,
    keys_promoted_gt1: u64,
}

fn make_policy(policy_name: &str, cap_bytes: u64) -> Box<dyn EvictionPolicy> {
    if policy_name.starts_with("s3-fifo") {
        let mut small = 0.10f64;
        let mut ghost = 0.90f64;
        let mut thresh = 1u8;
        for part in policy_name.split(':').skip(1) {
            if let Some(v) = part.strip_prefix("s=") { small = v.parse().unwrap(); }
            else if let Some(v) = part.strip_prefix("g=") { ghost = v.parse().unwrap(); }
            else if let Some(v) = part.strip_prefix("t=") { thresh = v.parse().unwrap(); }
        }
        Box::new(S3Fifo::with_params(cap_bytes, small, ghost, thresh))
    } else {
        match policy_name {
            "allkeys-lru" => Box::new(AllkeysLru::new(cap_bytes, 5, 42)),
            "allkeys-truelru" => Box::new(TrueLru::new(cap_bytes)),
            "allkeys-lfu" => Box::new(AllkeysLfu::new(cap_bytes, 5, 10, 1, 1000, 42)),
            "allkeys-random" => Box::new(AllkeysRandom::new(cap_bytes, 42)),
            "fifo" => Box::new(Fifo::new(cap_bytes)),
            _ => panic!("unknown policy: {policy_name}"),
        }
    }
}

fn make_promotion(name: &str) -> Box<dyn PromotionPolicy> {
    if let Some(n) = name.strip_prefix("second-hit-") {
        Box::new(SecondHit::new(n.parse().expect("bad second-hit window size")))
    } else if let Some(n) = name.strip_prefix("reuse-within-") {
        Box::new(RecentReuse::new(n.parse().expect("bad reuse-within threshold")))
    } else {
        match name {
            "always" => Box::new(AlwaysPromote),
            "never" => Box::new(NeverPromote),
            _ => panic!("unknown promotion policy: {name} (valid: always, never, second-hit-N, reuse-within-N)"),
        }
    }
}

fn make_admission(name: &str) -> Box<dyn AdmissionPolicy> {
    match name {
        "admit-dram" => Box::new(AdmitToDram),
        "admit-flash" => Box::new(AdmitToFlash),
        _ => panic!("unknown admission policy: {name} (valid: admit-dram, admit-flash)"),
    }
}

/// Two-tier simulation: DRAM (eviction policy) + Flash (infinite capacity).
///
/// Flow per access:
///   1. Key in DRAM? → DRAM hit, update policy metadata.
///   2. Key in Flash? → Flash hit, ask promotion policy.
///      - Promote: admit to DRAM (evictions go to flash), remove from flash.
///      - Transient: serve from flash, don't disturb DRAM.
///   3. Neither? → True miss. Admit to DRAM (evictions go to flash).
fn simulate_two_tier(
    trace: &Trace,
    cap_bytes: u64,
    cap_frac: f64,
    policy_name: &str,
    promotion_name: &str,
    admission_name: &str,
    mode: SpillMode,
    include_first_touch: bool,
    key_spill_overhead: u64,
) -> TaskResult {
    let policy_cap = match mode {
        SpillMode::KeySpilling => cap_bytes.saturating_sub(key_spill_overhead * trace.n_unique as u64),
        SpillMode::NoKeySpilling => cap_bytes.saturating_sub(trace.total_key_bytes),
    };

    let mut policy = make_policy(policy_name, policy_cap);
    let mut promo = make_promotion(promotion_name);
    let mut admit = make_admission(admission_name);

    // Flash tier: tracks keys that were evicted from DRAM
    let mut flash: FxHashSet<u64> = FxHashSet::default();
    // Track entry sizes for re-admission
    let mut entry_sizes: FxHashMap<u64, u64> = FxHashMap::default();

    let mut seen = FxHashSet::default();
    let mut obj_miss: u64 = 0;
    let mut byte_miss: u64 = 0;
    let mut measured: u64 = 0;
    let mut measured_bytes: u64 = 0;
    let mut flash_hits: u64 = 0;
    let mut promotions: u64 = 0;
    let mut evictions: u64 = 0;
    // Per-key churn: how many times each key was evicted/promoted
    let mut per_key_evictions: FxHashMap<u64, u32> = FxHashMap::default();
    let mut per_key_promotions: FxHashMap<u64, u32> = FxHashMap::default();

    for t in 0..trace.keys.len() {
        let k = trace.keys[t];
        let entry_size = match mode {
            SpillMode::KeySpilling => (trace.key_sizes[t] + trace.value_sizes[t]).saturating_sub(key_spill_overhead),
            SpillMode::NoKeySpilling => trace.value_sizes[t],
        };
        entry_sizes.insert(k, entry_size);

        let first_touch = seen.insert(k);

        // 1. Check DRAM
        if policy.contains(k) {
            policy.touch(k, t as u64);
            if first_touch && !include_first_touch { continue; }
            measured += 1;
            measured_bytes += trace.value_sizes[t];
            continue;
        }

        // 2. Check Flash
        if !first_touch && flash.contains(&k) {
            flash_hits += 1;
            if promo.should_promote(k, t as u64) {
                promotions += 1;
                *per_key_promotions.entry(k).or_insert(0) += 1;
                flash.remove(&k);
                let evicted = policy.admit(k, entry_size, t as u64);
                evictions += evicted.len() as u64;
                for ek in &evicted { *per_key_evictions.entry(*ek).or_insert(0) += 1; }
                for ek in evicted { flash.insert(ek); }
            }
            if first_touch && !include_first_touch { continue; }
            measured += 1;
            measured_bytes += trace.value_sizes[t];
            continue;
        }

        // 3. True miss — admission policy decides DRAM or flash
        if admit.admit_to_dram(k, t as u64) {
            let evicted = policy.admit(k, entry_size, t as u64);
            evictions += evicted.len() as u64;
            for ek in &evicted { *per_key_evictions.entry(*ek).or_insert(0) += 1; }
            for ek in evicted { flash.insert(ek); }
        } else {
            // Admit directly to flash
            flash.insert(k);
        }

        if first_touch && !include_first_touch { continue; }
        measured += 1;
        measured_bytes += trace.value_sizes[t];
        obj_miss += 1;
        byte_miss += trace.value_sizes[t];
    }

    // Per-key churn stats
    let keys_ever_evicted = per_key_evictions.len() as u64;
    let keys_ever_promoted = per_key_promotions.len() as u64;
    let max_evictions_per_key = per_key_evictions.values().copied().max().unwrap_or(0) as u64;
    let max_promotions_per_key = per_key_promotions.values().copied().max().unwrap_or(0) as u64;
    // Keys evicted more than once = "churning" keys
    let keys_evicted_gt1 = per_key_evictions.values().filter(|&&v| v > 1).count() as u64;
    let keys_promoted_gt1 = per_key_promotions.values().filter(|&&v| v > 1).count() as u64;

    let obj_miss_ratio = if measured > 0 { obj_miss as f64 / measured as f64 } else { f64::NAN };
    let byte_miss_ratio = if measured_bytes > 0 { byte_miss as f64 / measured_bytes as f64 } else { f64::NAN };
    let flash_hit_ratio = if measured > 0 { flash_hits as f64 / measured as f64 } else { 0.0 };

    TaskResult {
        workload: trace.workload.clone(),
        policy: policy_name.to_string(),
        promotion_policy: promo.label(),
        admission_policy: admit.label().to_string(),
        mode,
        cap_frac,
        cap_bytes,
        total_key_bytes: trace.total_key_bytes,
        total_value_bytes: trace.total_value_bytes,
        total_bytes: trace.total_bytes,
        n_unique: trace.n_unique,
        total_accesses: trace.keys.len(),
        obj_miss_ratio,
        byte_miss_ratio,
        flash_hit_ratio,
        promotions,
        flash_hits,
        evictions,
        keys_ever_evicted,
        keys_ever_promoted,
        max_evictions_per_key,
        max_promotions_per_key,
        keys_evicted_gt1,
        keys_promoted_gt1,
    }
}

fn main() {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.output).unwrap();

    let modes: Vec<SpillMode> = match cli.mode.as_str() {
        "key-spilling" => vec![SpillMode::KeySpilling],
        "no-key-spilling" => vec![SpillMode::NoKeySpilling],
        "both" => vec![SpillMode::KeySpilling, SpillMode::NoKeySpilling],
        _ => panic!("unknown mode: {} (valid: key-spilling, no-key-spilling, both)", cli.mode),
    };

    // Validate policies
    for p in &cli.policy {
        if !p.starts_with("s3-fifo") {
            let valid = ["allkeys-lru", "allkeys-truelru", "allkeys-lfu", "allkeys-random", "fifo"];
            assert!(valid.contains(&p.as_str()), "unknown policy: {p}");
        }
    }

    // Validate promotion policies
    for p in &cli.promotion {
        if !p.starts_with("second-hit-") && !p.starts_with("reuse-within-") {
            let valid = ["always", "never"];
            assert!(valid.contains(&p.as_str()), "unknown promotion policy: {p}");
        }
    }

    // Load traces
    let traces: Vec<Arc<Trace>> = cli.traces.iter().map(|p| {
        eprintln!("Loading {}...", p.display());
        let t = load_trace(p, cli.key_overhead);
        eprintln!("  {} events, {} unique keys, {} key bytes, {} value bytes, {} total bytes",
            t.keys.len(), t.n_unique, t.total_key_bytes, t.total_value_bytes, t.total_bytes);
        Arc::new(t)
    }).collect();

    // Build tasks: trace × policy × promotion × mode × capacity
    let mut tasks: Vec<(Arc<Trace>, String, String, String, f64, u64, SpillMode)> = Vec::new();
    for trace in &traces {
        for policy in &cli.policy {
            for promo in &cli.promotion {
                for admit in &cli.admission {
                    for &mode in &modes {
                        let (min_bytes, max_bytes) = match mode {
                            SpillMode::KeySpilling => (cli.key_spill_overhead * trace.n_unique as u64, trace.total_bytes),
                            SpillMode::NoKeySpilling => (trace.total_key_bytes, trace.total_bytes),
                        };
                        for i in 0..cli.cap_points {
                            let frac = if cli.cap_points > 1 { i as f64 / (cli.cap_points - 1) as f64 } else { 1.0 };
                            let cap_bytes = min_bytes + (frac * (max_bytes - min_bytes) as f64).round() as u64;
                            tasks.push((trace.clone(), policy.clone(), promo.clone(), admit.clone(), frac, cap_bytes, mode));
                        }
                    }
                }
            }
        }
    }

    eprintln!("Running {} tasks in parallel...", tasks.len());
    let include_ft = cli.include_first_touch;
    let kso = cli.key_spill_overhead;
    let start = std::time::Instant::now();

    let results: Vec<TaskResult> = tasks
        .par_iter()
        .map(|(trace, policy, promo, admit, frac, cap, mode)| {
            simulate_two_tier(trace, *cap, *frac, policy, promo, admit, *mode, include_ft, kso)
        })
        .collect();

    let elapsed = start.elapsed();
    eprintln!("Done in {:.1}s", elapsed.as_secs_f64());

    // Write CSV per policy × promotion × admission × mode
    for policy in &cli.policy {
        for promo in &cli.promotion {
            for admit in &cli.admission {
                for &mode in &modes {
                    let safe_policy = policy.replace('-', "_");
                    let safe_promo = promo.replace('-', "_");
                    let safe_admit = admit.replace('-', "_");
                    let safe_mode = mode.label().replace('-', "_");
                    let path = cli.output.join(format!("{safe_policy}_{safe_mode}_{safe_admit}_promo_{safe_promo}_mrc_curves.csv"));
                    let mut wtr = WriterBuilder::new().from_path(&path).unwrap();
                    wtr.write_record(["workload", "policy", "promotion_policy", "admission_policy", "mode", "measurement_mode",
                        "capacity_fraction", "capacity_bytes",
                        "total_key_bytes", "total_value_bytes", "total_bytes",
                        "unique_objects_in_trace",
                        "total_accesses", "object_miss_ratio", "byte_miss_ratio",
                        "flash_hit_ratio", "promotions", "flash_hits",
                        "evictions", "keys_ever_evicted", "keys_ever_promoted",
                        "max_evictions_per_key", "max_promotions_per_key",
                        "keys_evicted_gt1", "keys_promoted_gt1"]).unwrap();

                    for r in &results {
                        if r.policy != *policy || r.promotion_policy != *promo || r.admission_policy != *admit || r.mode != mode { continue; }
                        let mmode = if include_ft { "include-first-touch" } else { "exclude-first-touch" };
                        wtr.write_record(&[
                            &r.workload, &r.policy, &r.promotion_policy, &r.admission_policy, mode.label(), mmode,
                            &format!("{:.6}", r.cap_frac), &r.cap_bytes.to_string(),
                            &r.total_key_bytes.to_string(), &r.total_value_bytes.to_string(),
                            &r.total_bytes.to_string(), &r.n_unique.to_string(),
                            &r.total_accesses.to_string(),
                            &format!("{:.8}", r.obj_miss_ratio), &format!("{:.8}", r.byte_miss_ratio),
                            &format!("{:.8}", r.flash_hit_ratio),
                            &r.promotions.to_string(), &r.flash_hits.to_string(),
                            &r.evictions.to_string(), &r.keys_ever_evicted.to_string(),
                            &r.keys_ever_promoted.to_string(),
                            &r.max_evictions_per_key.to_string(), &r.max_promotions_per_key.to_string(),
                            &r.keys_evicted_gt1.to_string(), &r.keys_promoted_gt1.to_string(),
                        ]).unwrap();
                    }
                    wtr.flush().unwrap();
                    eprintln!("Wrote {}", path.display());
                }
            }
        }
    }

    // Combined CSV
    let combined_path = cli.output.join("promotion_sweep_combined.csv");
    let mut wtr = WriterBuilder::new().from_path(&combined_path).unwrap();
    wtr.write_record(["workload", "policy", "promotion_policy", "admission_policy", "mode", "measurement_mode",
        "capacity_fraction", "capacity_bytes",
        "total_key_bytes", "total_value_bytes", "total_bytes",
        "unique_objects_in_trace",
        "total_accesses", "object_miss_ratio", "byte_miss_ratio",
        "flash_hit_ratio", "promotions", "flash_hits",
        "evictions", "keys_ever_evicted", "keys_ever_promoted",
        "max_evictions_per_key", "max_promotions_per_key",
        "keys_evicted_gt1", "keys_promoted_gt1"]).unwrap();
    for r in &results {
        let mmode = if include_ft { "include-first-touch" } else { "exclude-first-touch" };
        wtr.write_record(&[
            &r.workload, &r.policy, &r.promotion_policy, &r.admission_policy, r.mode.label(), mmode,
            &format!("{:.6}", r.cap_frac), &r.cap_bytes.to_string(),
            &r.total_key_bytes.to_string(), &r.total_value_bytes.to_string(),
            &r.total_bytes.to_string(), &r.n_unique.to_string(),
            &r.total_accesses.to_string(),
            &format!("{:.8}", r.obj_miss_ratio), &format!("{:.8}", r.byte_miss_ratio),
            &format!("{:.8}", r.flash_hit_ratio),
            &r.promotions.to_string(), &r.flash_hits.to_string(),
            &r.evictions.to_string(), &r.keys_ever_evicted.to_string(),
            &r.keys_ever_promoted.to_string(),
            &r.max_evictions_per_key.to_string(), &r.max_promotions_per_key.to_string(),
            &r.keys_evicted_gt1.to_string(), &r.keys_promoted_gt1.to_string(),
        ]).unwrap();
    }
    wtr.flush().unwrap();
    eprintln!("Wrote {}", combined_path.display());
}
