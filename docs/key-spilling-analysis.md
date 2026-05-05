# Key Spilling MRC Analysis

## Overview

Compare MRC curves under two eviction models to quantify the DRAM cost/benefit of retaining keys in memory when values are evicted to storage.

## Eviction Models

### Key Spilling (current behavior)

When an entry is evicted, **both key and value** are removed from DRAM. On next access, the full key+value must be fetched from storage.

- Capacity budget: total bytes available for keys + values
- Eviction frees: `key_bytes + value_bytes` per entry
- X-axis range: `[0, Sum(all key_bytes + value_bytes)]`

### No Key Spilling (proposed)

When an entry is evicted, **only the value** is removed from DRAM. The key remains as fixed overhead. On next access, only the value is fetched from storage; the key is re-inserted into the eviction policy as a fresh entry (no state preserved).

- Capacity budget: total bytes, but `Sum(key_bytes)` is fixed (all keys always resident)
- Eviction frees: `value_bytes` only per entry
- Effective value budget at capacity C: `C - Sum(key_bytes)`
- X-axis range: `[Sum(key_bytes), Sum(all key_bytes + value_bytes)]`
- Hit/miss: defined by value presence only; key presence is fixed DRAM cost

## Key Insight

These are **separate simulations**, not a simple X-axis shift. The eviction pressure differs because:
- Key-spilling: evicting one entry frees `key_i + value_i` bytes
- No-key-spilling: evicting one entry frees only `value_i` bytes

At the same total DRAM budget, key-spilling can fit more values because each eviction reclaims more space. The curves will have different shapes, not just different offsets.

## Graph Specification

Single overlaid chart per policy:

```
Y-axis: miss ratio (%) — 0 to 100, tick marks every 5% or 10%
X-axis: DRAM capacity (bytes or % of total dataset) — 0 to Sum(key+value bytes)
         Tick marks at meaningful intervals for reading exact values

Curve 1 (key-spilling):
  - Starts at (0, 100%)
  - Ends at (Sum(key+value), 0%)
  - Label: "key-spilling"

Curve 2 (no-key-spilling):
  - Starts at (Sum(key_bytes), 100%)
  - Ends at (Sum(key+value), 0%)
  - Label: "no-key-spilling"
  - Vertical dashed line at X = Sum(key_bytes) marking the minimum DRAM floor
```

The horizontal gap between curves at any miss ratio Y shows the DRAM savings (or cost) of each approach.

## Trace Format Changes

### Synthetic Traces

Add key:value size ratio as a workload parameter:

```yaml
value_sizes:
  small_sizes: [64, 128, 256]
  medium_sizes: [512, 1024, 2048]
  large_sizes: [4096, 8192]
  # ...existing distribution...

key_value_ratio: "1:4"   # key_bytes = value_bytes / 4
# Examples: "1:1", "1:4", "1:10", "1:100"
```

Trace CSV columns: `key, op, key_size, value_size, workload_label`

### Valkey MONITOR TRACE

Already has `key_bytes` and `value_bytes` columns — no changes needed.

## Rust Simulator Changes

### New CLI flag

```bash
mrc-sim trace.csv -p s3-fifo -n 101 -o out/ --mode key-spilling    # default (current)
mrc-sim trace.csv -p s3-fifo -n 101 -o out/ --mode no-key-spilling  # new
mrc-sim trace.csv -p s3-fifo -n 101 -o out/ --mode both             # run both, output both CSVs
```

### Simulation differences

| | Key Spilling | No Key Spilling |
|---|---|---|
| Entry size in cache | `key_bytes + value_bytes` | `value_bytes` only |
| Bytes freed on eviction | `key_bytes + value_bytes` | `value_bytes` only |
| Capacity range | `[0, total_bytes]` | `[Sum(key_bytes), total_bytes]` |
| On re-access after eviction | Full miss (fetch key+value) | Value miss only (key in DRAM, re-insert fresh into policy) |
| Hit definition | key+value in DRAM | value in DRAM |

### Output CSV

Add column `mode` to distinguish:

```csv
workload,policy,mode,capacity_fraction,capacity_bytes,...,object_miss_ratio,byte_miss_ratio
trace,s3-fifo,key-spilling,0.10,43414885,...,0.075,0.068
trace,s3-fifo,no-key-spilling,0.10,43414885,...,0.092,0.084
```

## Analysis Questions

1. At what key:value ratio does key-spilling become worthwhile? (i.e., the DRAM savings from evicting keys exceeds the re-fetch cost)
2. How does the gap between curves change across policies? (S3-FIFO vs LRU vs LFU)
3. For real Valkey workloads (cluster52 trace), what is the actual key:value ratio and resulting DRAM impact?

## Implementation Plan

1. Update synthetic trace generation to emit `key_size` column based on ratio parameter
2. Add `--mode` flag to Rust simulator with no-key-spilling eviction logic
3. Add overlay plotting (both curves on single chart with vertical Km line)
4. Run analysis across policies × key:value ratios × workloads
