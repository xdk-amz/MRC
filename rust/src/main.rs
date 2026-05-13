// MRC Simulator — see DESIGN.md for capacity sweep semantics.
// Key invariant: no-key-spilling sweeps [total_key_bytes, total_bytes],
// policy_cap = cap_bytes - total_key_bytes. Never sweep below total_key_bytes.

use clap::Parser;
use csv::WriterBuilder;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::PathBuf;
use std::sync::Arc;

mod policies;
use policies::{AllkeysLfu, AllkeysLru, AllkeysRandom, Fifo, S3Fifo, EvictionPolicy};

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
#[command(name = "mrc-sim", about = "MRC simulator for Valkey eviction policies")]
struct Cli {
    /// Trace CSV files
    #[arg(required = true)]
    traces: Vec<PathBuf>,

    /// Policies: allkeys-lru, allkeys-lfu, allkeys-random, fifo, s3-fifo
    #[arg(short, long, value_delimiter = ',', default_value = "allkeys-lru")]
    policy: Vec<String>,

    /// Number of capacity points
    #[arg(short = 'n', long, default_value = "21")]
    cap_points: usize,

    /// Output directory
    #[arg(short, long, default_value = "out/valkey_mrc")]
    output: PathBuf,

    /// Spill mode: key-spilling, no-key-spilling, both
    #[arg(short, long, default_value = "both")]
    mode: String,

    /// Fixed per-key DRAM overhead in bytes (dictEntry + robj, not captured in trace)
    #[arg(long, default_value = "0")]
    key_overhead: u64,

    /// Include first-touch accesses in miss ratio (counts compulsory misses)
    #[arg(long, default_value = "false")]
    include_first_touch: bool,

    /// Per-key DRAM overhead for storage index when key is spilled (key-spilling mode only)
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

    // Skip first line if it's not data (no commas, or starts with known header)
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
        if i == 0 && skip_first {
            continue;
        }
        let ncols = record.len();

        if ncols >= 10 {
            // Valkey MONITOR TRACE: ts_us, seq, db_id, cmd, key(b64), access_type, key_exists, obj_type, key_bytes, value_bytes
            let key_str = &record[4];
            let key_id = *key_map.entry(key_str.to_string()).or_insert_with(|| {
                let id = next_id;
                next_id += 1;
                id
            });
            let kb: u64 = record[8].parse().unwrap_or(0);
            let vb: u64 = record[9].parse().unwrap_or(0);
            keys.push(key_id);
            key_sizes.push(kb);
            value_sizes.push(vb);
        } else if ncols >= 6 {
            // New synthetic: t, op, key, key_size, value_size, workload_label
            keys.push(record[2].parse::<u64>().expect("bad key"));
            key_sizes.push(record[3].parse::<u64>().expect("bad key_size"));
            value_sizes.push(record[4].parse::<u64>().expect("bad value_size"));
        } else if ncols >= 5 {
            // Old synthetic: t, op, key, value_size, workload_label (no key_size)
            keys.push(record[2].parse::<u64>().expect("bad key"));
            key_sizes.push(0);
            value_sizes.push(record[3].parse::<u64>().expect("bad size"));
        } else {
            // Minimal: key, value_size
            keys.push(record[0].parse::<u64>().expect("bad key"));
            key_sizes.push(0);
            value_sizes.push(record[1].parse::<u64>().expect("bad size"));
        }
    }

    // Add per-key overhead to key sizes
    if key_overhead > 0 {
        for ks in key_sizes.iter_mut() {
            *ks += key_overhead;
        }
    }

    // Compute unique key/value bytes (first occurrence of each key)
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
}

fn make_policy(policy_name: &str, cap_bytes: u64) -> Box<dyn EvictionPolicy> {
    if policy_name.starts_with("s3-fifo") {
        let mut small = 0.10f64;
        let mut ghost = 0.90f64;
        let mut thresh = 1u8;
        for part in policy_name.split(':').skip(1) {
            if let Some(v) = part.strip_prefix("s=") {
                small = v.parse().unwrap();
            } else if let Some(v) = part.strip_prefix("g=") {
                ghost = v.parse().unwrap();
            } else if let Some(v) = part.strip_prefix("t=") {
                thresh = v.parse().unwrap();
            }
        }
        Box::new(S3Fifo::with_params(cap_bytes, small, ghost, thresh))
    } else {
        match policy_name {
            "allkeys-lru" => Box::new(AllkeysLru::new(cap_bytes, 5, 42)),
            "allkeys-lfu" => Box::new(AllkeysLfu::new(cap_bytes, 5, 10, 1, 1000, 42)),
            "allkeys-random" => Box::new(AllkeysRandom::new(cap_bytes, 42)),
            "fifo" => Box::new(Fifo::new(cap_bytes)),
            _ => panic!("unknown policy: {policy_name}"),
        }
    }
}

fn simulate_one(
    trace: &Trace,
    cap_bytes: u64,
    cap_frac: f64,
    policy_name: &str,
    mode: SpillMode,
    include_first_touch: bool,
    key_spill_overhead: u64,
) -> TaskResult {
    // cap_bytes = total DRAM budget; for no-key-spilling, subtract fixed key cost
    // For key-spilling, subtract the irreducible index overhead (always resident)
    let policy_cap = match mode {
        SpillMode::KeySpilling => cap_bytes.saturating_sub(key_spill_overhead * trace.n_unique as u64),
        SpillMode::NoKeySpilling => cap_bytes.saturating_sub(trace.total_key_bytes),
    };
    let mut policy = make_policy(policy_name, policy_cap);

    let mut seen = FxHashSet::default();
    let mut obj_miss: u64 = 0;
    let mut byte_miss: u64 = 0;
    let mut measured: u64 = 0;
    let mut measured_bytes: u64 = 0;

    for t in 0..trace.keys.len() {
        let k = trace.keys[t];
        let entry_size = match mode {
            SpillMode::KeySpilling => (trace.key_sizes[t] + trace.value_sizes[t]).saturating_sub(key_spill_overhead),
            SpillMode::NoKeySpilling => trace.value_sizes[t],
        };
        let first_touch = seen.insert(k);
        let hit = policy.access(k, entry_size, t as u64);
        if first_touch && !include_first_touch {
            continue;
        }
        measured += 1;
        measured_bytes += trace.value_sizes[t];
        if !hit {
            obj_miss += 1;
            byte_miss += trace.value_sizes[t];
        }
    }

    let obj_miss_ratio = if measured > 0 { obj_miss as f64 / measured as f64 } else { f64::NAN };
    let byte_miss_ratio = if measured_bytes > 0 { byte_miss as f64 / measured_bytes as f64 } else { f64::NAN };

    TaskResult {
        workload: trace.workload.clone(),
        policy: policy_name.to_string(),
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
            let valid = ["allkeys-lru", "allkeys-lfu", "allkeys-random", "fifo"];
            assert!(valid.contains(&p.as_str()), "unknown policy: {p}");
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

    // Build tasks
    let mut tasks: Vec<(Arc<Trace>, String, f64, u64, SpillMode)> = Vec::new();
    for trace in &traces {
        for policy in &cli.policy {
            for &mode in &modes {
                // Capacity sweep range depends on mode
                // key-spilling: 0 -> total (keys can be evicted)
                // no-key-spilling: total_key -> total (keys always resident)
                let (min_bytes, max_bytes) = match mode {
                    SpillMode::KeySpilling => (cli.key_spill_overhead * trace.n_unique as u64, trace.total_bytes),
                    SpillMode::NoKeySpilling => (trace.total_key_bytes, trace.total_bytes),
                };
                for i in 0..cli.cap_points {
                    let frac = if cli.cap_points > 1 { i as f64 / (cli.cap_points - 1) as f64 } else { 1.0 };
                    let cap_bytes = min_bytes + (frac * (max_bytes - min_bytes) as f64).round() as u64;
                    tasks.push((trace.clone(), policy.clone(), frac, cap_bytes, mode));
                }
            }
        }
    }

    eprintln!("Running {} tasks in parallel...", tasks.len());
    let start = std::time::Instant::now();

    let results: Vec<TaskResult> = tasks
        .par_iter()
        .map(|(trace, policy, frac, cap, mode)| simulate_one(trace, *cap, *frac, policy, *mode, cli.include_first_touch, cli.key_spill_overhead))
        .collect();

    let elapsed = start.elapsed();
    eprintln!("Done in {:.1}s", elapsed.as_secs_f64());

    // Write CSV per policy per mode
    for policy in &cli.policy {
        for &mode in &modes {
            let safe_policy = policy.replace('-', "_");
            let safe_mode = mode.label().replace('-', "_");
            let path = cli.output.join(format!("{safe_policy}_{safe_mode}_mrc_curves.csv"));
            let mut wtr = WriterBuilder::new().from_path(&path).unwrap();
            wtr.write_record(["workload", "policy", "mode", "measurement_mode",
                "capacity_fraction", "capacity_bytes",
                "total_key_bytes", "total_value_bytes", "total_bytes",
                "unique_objects_in_trace",
                "total_accesses", "object_miss_ratio", "byte_miss_ratio"]).unwrap();

            for r in &results {
                if r.policy != *policy || r.mode != mode { continue; }
                wtr.write_record(&[
                    &r.workload, &r.policy, mode.label(), &(if cli.include_first_touch { "include-first-touch".to_string() } else { "exclude-first-touch".to_string() }),
                    &format!("{:.6}", r.cap_frac), &r.cap_bytes.to_string(),
                    &r.total_key_bytes.to_string(), &r.total_value_bytes.to_string(),
                    &r.total_bytes.to_string(),
                    &r.n_unique.to_string(),
                    &r.total_accesses.to_string(),
                    &format!("{:.8}", r.obj_miss_ratio), &format!("{:.8}", r.byte_miss_ratio),
                ]).unwrap();
            }
            wtr.flush().unwrap();
            eprintln!("Wrote {}", path.display());
        }
    }
}
