# Valkey Tiering MRC

Miss Ratio Curve (MRC) generation toolkit for evaluating Valkey eviction policies and DRAM tiering configurations.

## Architecture

Two independent tools:

| Tool | Language | Purpose |
|------|----------|---------|
| `mrc-sim` | Rust | Fast multi-policy MRC computation (5 eviction policies, parallel) |
| `valkey-tiering-mrc` | Python | Synthetic trace generation + MRC plotting |

## Directory Structure

```
.
├── rust/                    # Rust MRC simulator
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # CLI, trace loading, parallel orchestration
│       └── policies.rs      # allkeys-lru, allkeys-lfu, allkeys-random, fifo, s3-fifo
├── python/                  # Python trace gen + plotting
│   ├── pyproject.toml
│   ├── examples/
│   │   └── default_config.yaml
│   ├── src/valkey_tiering_mrc/
│   │   ├── cli.py           # CLI: generate-traces, plot
│   │   ├── config.py        # YAML config loading
│   │   ├── traces.py        # Synthetic + Valkey MONITOR TRACE loaders
│   │   ├── plot.py          # MRC charting (forward, inverse, contact sheets)
│   │   └── pipeline.py      # Orchestration
│   └── tests/
│       └── test_traces.py
├── docs/charts/             # Pre-generated reference charts
└── README.md
```

## Rust MRC Simulator

Computes MRC curves for 5 eviction policies using sampling-based simulation matching Valkey OSS behavior.

### Build

```bash
cd rust
cargo build --release
```

### Usage

```bash
# From a Valkey MONITOR TRACE CSV or synthetic trace
rust/target/release/mrc-sim trace.csv \
    -p allkeys-lru,allkeys-lfu,allkeys-random,fifo,s3-fifo \
    -n 101 \
    -o output/

# Single policy, more capacity points
rust/target/release/mrc-sim trace.csv -p s3-fifo -n 1001 -o output/
```

Output: one CSV per policy with columns `capacity_fraction_of_unique_bytes`, `object_miss_ratio`, `byte_miss_ratio`, etc.

### Supported Policies

- **allkeys-lru** — Approximate LRU with N=5 sampling (Valkey default)
- **allkeys-lfu** — Approximate LFU with Morris counters + decay
- **allkeys-random** — Random eviction
- **fifo** — First-in first-out
- **s3-fifo** — Three-queue FIFO with ghost filter (scan-resistant)

## Python Trace Generator & Plotter

### Install

```bash
cd python
pip install -e .
```

### Generate Synthetic Traces

```bash
valkey-tiering-mrc generate-traces --config examples/default_config.yaml --out traces/
```

Generates 8 workload patterns (zipfian, moving window, rotating hot sets, etc.) as CSVs compatible with `mrc-sim`.

### Plot MRC Curves

```bash
valkey-tiering-mrc plot --curves output/forward_mrc.csv --inverse output/inverse_mrc.csv --out plots/
```

### Trace Formats

The Rust simulator accepts two CSV formats:

1. **Synthetic** (from Python generator): `key,op,value_size,workload_label`
2. **Valkey MONITOR TRACE**: 10-column format with `ts_us,seq,db_id,cmd,key,access_type,key_exists,obj_type,key_bytes,value_bytes`

Both are auto-detected.

## Workflow

```bash
# 1. Generate synthetic traces
valkey-tiering-mrc generate-traces --config python/examples/default_config.yaml --out traces/

# 2. Compute MRC for all policies
rust/target/release/mrc-sim traces/stable_zipfian_hot_set.csv -p allkeys-lru,s3-fifo -n 101 -o results/

# 3. Plot results
valkey-tiering-mrc plot --curves results/forward.csv --inverse results/inverse.csv --out plots/
```

Or with a real Valkey trace:

```bash
# Capture trace from running Valkey instance
valkey-cli MONITOR TRACE RATE 0.1 SEED 42 > trace.csv

# Compute MRC
rust/target/release/mrc-sim trace.csv -n 101 -o results/
```
