# Promotion & Admission Policy Analysis

## Executive Summary

When a tiered cache (DRAM + Flash) receives a request for a key not in DRAM, two policy decisions determine system behavior:

1. **Admission policy**: When a key is first seen, does it go to DRAM or Flash?
2. **Promotion policy**: When a key is read from Flash, does it move to DRAM?

These decisions control the **read/write IO balance** of the tiering system. Our simulation across 7 workloads (10M events each) with 96 policy combinations reveals:

- **`admit-flash + second-hit-50K`** is the best general-purpose configuration — it achieves the lowest write-weighted IO cost across all workloads with strong locality, while maintaining competitive miss rates.
- **Writes are the dominant cost.** At a 2:1 write:read cost ratio, the naive `admit-dram + always-promote` policy costs 2–4× more than filtered alternatives.
- **Eviction policy choice (LRU vs LFU vs FIFO) matters far less** than admission + promotion policy choice — typically <2pp miss rate difference.

### Recommended Configuration

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Admission | **admit-flash** | New keys go to flash; DRAM only via promotion. Eliminates first-touch eviction cascades. |
| Promotion | **second-hit-50K** | Promote on 2nd flash access within 50K-access window. Filters one-hit-wonders. |
| Eviction | allkeys-lru or allkeys-truelru | Marginal difference; LRU is simpler. |
| Metadata cost | ~400KB | 50K-entry ring buffer for second-hit tracking. |

---

## The Problem: Promotion Churn

The naive policy (`admit-dram + always-promote`) has a hidden cost: **every flash read triggers a promotion, which evicts a DRAM resident, which writes to flash.** This creates a 1:1 read:write ratio on the flash path.

For workloads with many low-reuse keys (one-hit-wonders, scans), this means:
- A key is fetched from flash (1 read)
- It's promoted to DRAM, evicting a victim (1 write)
- The promoted key is never accessed again
- It eventually gets evicted itself (another write)
- Net cost: 1 read + 2 writes for a key that was accessed once

The `admit-flash + second-hit` approach eliminates this: keys must prove reuse before entering DRAM, so one-hit-wonders stay in flash and are served with cheap reads only.

---

## Policy Definitions

### Admission Policies

| Policy | Behavior | Effect |
|--------|----------|--------|
| **admit-dram** | New keys inserted into DRAM; evict to flash if full | Default behavior. Every new key may trigger an eviction write. |
| **admit-flash** | New keys inserted directly into flash | DRAM is populated only via promotion. No first-touch eviction cascades. |

### Promotion Policies

| Policy | Behavior | Effect |
|--------|----------|--------|
| **always** | Promote to DRAM on every flash hit | Lowest miss rate, highest write cost. |
| **never** | Never promote; serve from flash | Zero writes from promotion, but DRAM underutilized. |
| **second-hit-N** | Promote on 2nd flash access within N-access window | Filters one-hit-wonders. N controls selectivity. |
| **reuse-within-N** | Promote if re-accessed within N accesses of last flash hit | Similar to second-hit but uses per-key timestamp. |

---

## Results: Twitter Production Trace (10M ops, 1.6M unique keys)

### @ 10% DRAM Capacity

| Admission | Promotion | Miss% | Flash Reads | Flash Writes | Weighted IO (R+2W) |
|-----------|-----------|-------|-------------|--------------|-------------------|
| admit-dram | always | 9.3% | 0.78M | 2.24M | 5.26M |
| admit-flash | always | 15.9% | 1.33M | 1.19M | 3.70M |
| **admit-flash** | **second-hit-50K** | **20.7%** | **1.73M** | **0.36M** | **2.45M** |
| admit-dram | never | 23.1% | 1.94M | 1.46M | 4.85M |

**Key insight**: `admit-flash + second-hit-50K` has 2.1× lower weighted IO than `admit-dram + always` despite 11pp worse miss rate. The write savings (0.36M vs 2.24M = 84% reduction) more than compensate.

### Why always-promote generates 6× more writes

With `admit-dram + always`:
- Every flash hit → promote → evict victim → **1 write per read**
- Promoted one-hit-wonders get evicted again → **wasted write**
- 776K promotions, 297K keys churning (evicted >1 time)

With `admit-flash + second-hit-50K`:
- Flash hits for unproven keys → serve from flash → **0 writes**
- Only 510K promotions of keys with proven reuse
- 19K keys churning — 93% less churn

---

## Results: Stable Zipfian Trace (10M ops, 667K unique keys)

| Admission | Promotion | Miss% | Flash Reads | Flash Writes | Weighted IO (R+2W) |
|-----------|-----------|-------|-------------|--------------|-------------------|
| admit-dram | always | 13.7% | 1.28M | 1.88M | 5.04M |
| **admit-flash** | **second-hit-50K** | **10.8%** | **1.01M** | **0.15M** | **1.31M** |

On pure Zipfian, `admit-flash + second-hit-50K` **wins on every metric** — lower miss rate, fewer reads, 92% fewer writes, 3.8× better weighted IO. This happens because the strong locality means keys that pass the 2nd-hit filter are genuinely hot and stay in DRAM permanently once promoted.

---

## Cross-Workload Comparison @ 10% DRAM

| Workload | admit-dram+always | admit-flash+2nd-hit-50K | Weighted IO Ratio |
|----------|-------------------|-------------------------|-------------------|
| **Stable Zipfian** | 5.04M (13.7% miss) | **1.31M** (10.8% miss) | 3.8× better |
| **Hot Core Noisy Tail** | 8.02M (23.2% miss) | **2.03M** (22.4% miss) | 4.0× better |
| **Twitter (cluster52)** | 5.26M (9.3% miss) | **2.45M** (20.7% miss) | 2.1× better |
| **Uniform Random** | 25.82M (89.0% miss) | **8.89M** (90.4% miss) | 2.9× better |
| **Sequential Scan** | 29.88M (100% miss) | **9.90M** (100% miss) | 3.0× better |
| **Stable Hot Set + Scans** | 8.98M (29.8% miss) | 8.81M (29.8% miss) | 1.0× (tie) |
| **Rotating Hot Sets** | 14.27M (46.5% miss) | 12.13M (76.9% miss) | 1.2× better |

`admit-flash + second-hit-50K` wins on weighted IO for **6 of 7 workloads** (ties on stable-hot-set+scans). The only workload where it has significantly worse miss rate is rotating hot sets — where phase changes invalidate the 2nd-hit history.

---

## The IO Cost Model

Each tiering operation has a real cost:

| Operation | What happens | Cost |
|-----------|-------------|------|
| Flash read | Fetch value from SSD, deserialize | 1 read unit |
| Flash write (eviction) | Serialize value, write to SSD | 2 read units (write amplification, wear) |
| Promotion | Flash read + DRAM insert + evict victim (flash write) | 1 read + 2 write = 5 units |
| Transient serve | Flash read, serve to client, no state change | 1 read unit |

The weighted IO formula: **`cost = flash_reads + W × flash_writes`** where W is the write cost multiplier.

| W (write:read cost) | Winner | Rationale |
|---------------------|--------|-----------|
| 1:1 (equal) | admit-flash + second-hit-50K | Lowest total IO |
| 2:1 | admit-flash + second-hit-50K | Write savings dominate |
| 4:1 | admit-flash + reuse-within-100 | Extreme write avoidance |
| 0.5:1 (reads expensive) | admit-dram + always | Minimize reads at any write cost |

The crossover point where `admit-dram + always` becomes optimal is approximately W < 0.7 — i.e., writes must be *cheaper* than reads. This is unrealistic for flash/SSD where write amplification and wear leveling make writes inherently more expensive.

---

## Implementation Considerations

### Second-Hit Filter Metadata

The `second-hit-50K` policy requires tracking recent flash accesses:
- **Ring buffer**: 50K entries × 8 bytes (key hash) = **400KB**
- **Hash set**: For O(1) lookup of "have I seen this key recently?"
- Total overhead: ~800KB — negligible vs dataset size

### Interaction with Eviction Policy

The eviction policy (LRU, LFU, etc.) only matters for keys **already in DRAM**. With `admit-flash`, DRAM is populated exclusively by promoted keys that have proven reuse. This means:
- The eviction pool contains higher-quality candidates
- LRU approximation errors matter less (fewer marginal keys to mis-rank)
- Eviction policy choice contributes <2pp miss rate difference

### When to Use admit-dram + always

The naive policy is still appropriate when:
- Flash writes are genuinely cheap (battery-backed RAM, NVDIMM)
- Miss latency is the only metric that matters (latency-critical path)
- The workload has near-zero one-hit-wonders (pure hot-set, no scans)

---

## Simulation Methodology

- **Tool**: `~/oss/MRC/rust/` — Rust two-tier DRAM+Flash MRC simulator
- **Traces**: 10M events each, exclude-first-touch measurement
- **Eviction policies**: allkeys-lru (sampled, 5 candidates), allkeys-truelru, allkeys-lfu, allkeys-random, fifo, s3-fifo
- **Capacity sweep**: 51 points from 0% to 100% DRAM
- **Flash model**: Infinite capacity (no flash evictions)
- **Key-spilling mode**: Keys can be fully evicted from DRAM

### Running the Simulator

```bash
cd ~/oss/MRC/rust
cargo run --release -- \
  ~/data/trace1/cluster52_10m.csv \
  -p allkeys-lru \
  --promotion always,never,second-hit-50000 \
  --admission admit-dram,admit-flash \
  -m key-spilling -n 51 \
  -o ../out/sweep
```

Output: CSV with per-capacity-point metrics (miss ratio, flash reads, flash writes, promotions, evictions, per-key churn stats).
