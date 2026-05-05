use clap::Parser;
use csv::WriterBuilder;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::PathBuf;
use std::sync::Arc;

mod policies;
use policies::{AllkeysLfu, AllkeysLru, AllkeysRandom, Fifo, S3Fifo, EvictionPolicy};

#[derive(Parser)]
#[command(name = "mrc-sim", about = "MRC simulator for Valkey eviction policies")]
struct Cli {
    /// Trace CSV files
    #[arg(required = true)]
    traces: Vec<PathBuf>,

    /// Policies: allkeys-lru, allkeys-lfu
    #[arg(short, long, value_delimiter = ',', default_value = "allkeys-lru")]
    policy: Vec<String>,

    /// Number of capacity points
    #[arg(short = 'n', long, default_value = "21")]
    cap_points: usize,

    /// Output directory
    #[arg(short, long, default_value = "out/valkey_mrc")]
    output: PathBuf,
}

struct Trace {
    workload: String,
    keys: Vec<u64>,
    sizes: Vec<u64>,
    unique_bytes: u64,
    n_unique: usize,
}

fn load_trace(path: &PathBuf) -> Trace {
    let workload = path.file_stem().unwrap().to_string_lossy().to_string();

    // Read first line to detect format
    let first_line = {
        let file = std::fs::File::open(path).expect("cannot open trace");
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        line.trim().to_string()
    };

    // Skip non-CSV header lines (e.g. "OK")
    let skip_first = !first_line.contains(',');

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .expect("cannot open trace");

    let mut keys = Vec::new();
    let mut sizes = Vec::new();
    let mut key_map: FxHashMap<String, u64> = FxHashMap::default();
    let mut next_id: u64 = 0;

    for (i, result) in rdr.records().enumerate() {
        let record = result.expect("bad csv row");
        if i == 0 && skip_first {
            continue;
        }
        let ncols = record.len();

        if ncols >= 10 {
            // New Valkey MONITOR TRACE format:
            // ts_us, seq, db_id, cmd, key(b64), access_type, key_exists, obj_type, key_bytes, value_bytes
            let key_str = &record[4];
            let key_id = *key_map.entry(key_str.to_string()).or_insert_with(|| {
                let id = next_id;
                next_id += 1;
                id
            });
            let value_bytes: u64 = record[9].parse().unwrap_or(0);
            keys.push(key_id);
            sizes.push(value_bytes);
        } else {
            // Old synthetic format: t, op, key(int), value_size, workload_label
            keys.push(record[2].parse::<u64>().expect("bad key"));
            sizes.push(record[3].parse::<u64>().expect("bad size"));
        }
    }

    let mut first_size: FxHashMap<u64, u64> = FxHashMap::default();
    for i in 0..keys.len() {
        first_size.entry(keys[i]).or_insert(sizes[i]);
    }
    let unique_bytes: u64 = first_size.values().sum();
    let n_unique = first_size.len();
    Trace { workload, keys, sizes, unique_bytes, n_unique }
}

struct TaskResult {
    workload: String,
    policy: String,
    cap_frac: f64,
    cap_bytes: u64,
    unique_bytes: u64,
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
) -> TaskResult {
    let mut policy = make_policy(policy_name, cap_bytes);

    let mut seen = FxHashSet::default();
    let mut obj_miss: u64 = 0;
    let mut byte_miss: u64 = 0;
    let mut measured: u64 = 0;
    let mut measured_bytes: u64 = 0;

    for t in 0..trace.keys.len() {
        let k = trace.keys[t];
        let sz = trace.sizes[t];
        let first_touch = seen.insert(k);
        let hit = policy.access(k, sz, t as u64);
        if first_touch {
            continue;
        }
        measured += 1;
        measured_bytes += sz;
        if !hit {
            obj_miss += 1;
            byte_miss += sz;
        }
    }

    let obj_miss_ratio = if measured > 0 { obj_miss as f64 / measured as f64 } else { f64::NAN };
    let byte_miss_ratio = if measured_bytes > 0 { byte_miss as f64 / measured_bytes as f64 } else { f64::NAN };

    TaskResult {
        workload: trace.workload.clone(),
        policy: policy_name.to_string(),
        cap_frac,
        cap_bytes,
        unique_bytes: trace.unique_bytes,
        n_unique: trace.n_unique,
        total_accesses: trace.keys.len(),
        obj_miss_ratio,
        byte_miss_ratio,
    }
}

fn main() {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.output).unwrap();

    // Validate non-parameterized policies
    for p in &cli.policy {
        if !p.starts_with("s3-fifo") {
            let valid = ["allkeys-lru", "allkeys-lfu", "allkeys-random", "fifo"];
            assert!(valid.contains(&p.as_str()), "unknown policy: {p}");
        }
    }

    // Load all traces
    let traces: Vec<Arc<Trace>> = cli.traces.iter().map(|p| {
        eprintln!("Loading {}...", p.display());
        let t = load_trace(p);
        eprintln!("  {} events, {} unique keys, {} unique bytes", t.keys.len(), t.n_unique, t.unique_bytes);
        Arc::new(t)
    }).collect();

    // Build task list: (trace_idx, policy, cap_frac, cap_bytes)
    let mut tasks: Vec<(Arc<Trace>, String, f64, u64)> = Vec::new();
    for trace in &traces {
        for policy in &cli.policy {
            for i in 0..cli.cap_points {
                let frac = if cli.cap_points > 1 { i as f64 / (cli.cap_points - 1) as f64 } else { 1.0 };
                let cap_bytes = (frac * trace.unique_bytes as f64).round() as u64;
                tasks.push((trace.clone(), policy.clone(), frac, cap_bytes));
            }
        }
    }

    eprintln!("Running {} tasks in parallel...", tasks.len());
    let start = std::time::Instant::now();

    let results: Vec<TaskResult> = tasks
        .par_iter()
        .map(|(trace, policy, frac, cap)| simulate_one(trace, *cap, *frac, policy))
        .collect();

    let elapsed = start.elapsed();
    eprintln!("Done in {:.1}s", elapsed.as_secs_f64());

    // Write CSV per policy
    for policy in &cli.policy {
        let safe = policy.replace('-', "_");
        let path = cli.output.join(format!("{safe}_mrc_curves.csv"));
        let mut wtr = WriterBuilder::new().from_path(&path).unwrap();
        wtr.write_record(["workload", "policy", "measurement_mode",
            "capacity_fraction_of_unique_bytes", "capacity_bytes",
            "unique_objects_in_trace", "unique_value_bytes_in_trace",
            "total_accesses", "object_miss_ratio", "byte_miss_ratio"]).unwrap();

        for r in &results {
            if r.policy != *policy { continue; }
            wtr.write_record(&[
                &r.workload, &r.policy, "exclude-first-touch",
                &format!("{:.6}", r.cap_frac), &r.cap_bytes.to_string(),
                &r.n_unique.to_string(), &r.unique_bytes.to_string(),
                &r.total_accesses.to_string(),
                &format!("{:.8}", r.obj_miss_ratio), &format!("{:.8}", r.byte_miss_ratio),
            ]).unwrap();
        }
        wtr.flush().unwrap();
        eprintln!("Wrote {}", path.display());
    }
}
