"""Synthetic trace generation for MRC experiments.

Each trace is a CSV with columns:
    t, op, key, value_size, workload_label

All ops are GET. Value sizes come from a deterministic key->size map so that
the same key always has the same value_size across traces.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np
import pandas as pd

from .config import HarnessConfig, ValueSizesConfig


# ---------------------------------------------------------------------------
# Deterministic value-size mapping
# ---------------------------------------------------------------------------

# 64-bit odd multiplicative constant (golden-ratio derived). Same for every
# trace so the same key id always maps to the same value size.
_HASH_MUL = np.uint64(0x9E3779B97F4A7C15)
_HASH_ADD = np.uint64(0xDA942042E4DD58B5)


def _hash_keys(keys: np.ndarray) -> np.ndarray:
    """64-bit multiplicative hash of int keys -> uint64 array."""
    k = keys.astype(np.uint64, copy=False)
    # numpy uint64 arithmetic naturally wraps mod 2**64
    with np.errstate(over="ignore"):
        h = k * _HASH_MUL + _HASH_ADD
        h ^= h >> np.uint64(33)
        h *= _HASH_MUL
        h ^= h >> np.uint64(29)
    return h


def value_sizes_for_keys(keys: np.ndarray, vs: ValueSizesConfig) -> np.ndarray:
    """Vectorized deterministic key-id -> value_size mapping.

    Buckets:
      bucket < 192      : small  (~75%)
      192 <= bucket<243 : medium (~20%)
      bucket >= 243     : large  (~5%)
    """
    if not vs.small_sizes or not vs.medium_sizes or not vs.large_sizes:
        raise ValueError("value_sizes config requires small/medium/large arrays")
    keys = np.asarray(keys, dtype=np.int64)
    h = _hash_keys(keys)
    bucket = ((h >> np.uint64(56)) & np.uint64(0xFF)).astype(np.int64)
    sub = ((h >> np.uint64(40)) & np.uint64(0xFFFF)).astype(np.int64)

    small = np.asarray(vs.small_sizes, dtype=np.int64)
    medium = np.asarray(vs.medium_sizes, dtype=np.int64)
    large = np.asarray(vs.large_sizes, dtype=np.int64)

    out = np.empty(keys.shape[0], dtype=np.int64)
    is_large = bucket >= 243
    is_medium = (bucket >= 192) & (~is_large)
    is_small = ~(is_large | is_medium)

    if is_small.any():
        out[is_small] = small[sub[is_small] % small.shape[0]]
    if is_medium.any():
        out[is_medium] = medium[sub[is_medium] % medium.shape[0]]
    if is_large.any():
        out[is_large] = large[sub[is_large] % large.shape[0]]
    return out


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _zipf_finite(rng: np.random.Generator, n: int, support: int, alpha: float) -> np.ndarray:
    """Sample n keys from a finite Zipf distribution with support 1..support.

    Returns 0-based key ids in [0, support).
    """
    if support <= 0:
        raise ValueError("support must be > 0")
    ranks = np.arange(1, support + 1, dtype=np.float64)
    weights = ranks ** (-float(alpha))
    weights /= weights.sum()
    cdf = np.cumsum(weights)
    cdf[-1] = 1.0  # guard against floating point drift
    u = rng.random(n)
    idx = np.searchsorted(cdf, u, side="right")
    np.clip(idx, 0, support - 1, out=idx)
    return idx.astype(np.int64)


# ---------------------------------------------------------------------------
# Workload generators. Each returns (keys, labels) numpy arrays.
# ---------------------------------------------------------------------------

@dataclass
class GenContext:
    rng: np.random.Generator
    events: int
    keyspace: int


def _gen_uniform_random(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    keys = ctx.rng.integers(0, ctx.keyspace, size=ctx.events, dtype=np.int64)
    labels = np.full(ctx.events, "uniform", dtype=object)
    return keys, labels


def _gen_moving_hot_window(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    window_size = int(params.get("window_size", 50_000))
    move_every = int(params.get("move_every_events", 1_000))
    step = int(params.get("window_step", 500))
    hot_p = float(params.get("hot_probability", 0.95))

    idx = np.arange(ctx.events, dtype=np.int64)
    win_idx = idx // max(move_every, 1)
    starts = (win_idx * step) % ctx.keyspace
    hot_off = ctx.rng.integers(0, max(window_size, 1), size=ctx.events, dtype=np.int64)
    hot_keys = (starts + hot_off) % ctx.keyspace
    rand_keys = ctx.rng.integers(0, ctx.keyspace, size=ctx.events, dtype=np.int64)
    hot_mask = ctx.rng.random(ctx.events) < hot_p
    keys = np.where(hot_mask, hot_keys, rand_keys)
    labels = np.where(hot_mask, "moving_window_hot", "background_random").astype(object)
    return keys, labels


def _gen_stable_zipfian_hot_set(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    alpha = float(params.get("zipf_alpha", 1.05))
    keys = _zipf_finite(ctx.rng, ctx.events, ctx.keyspace, alpha)
    labels = np.full(ctx.events, "stable_zipf", dtype=object)
    return keys, labels


def _gen_stable_hot_set_plus_scans(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    hot_keyspace = int(params.get("hot_keyspace", 20_000))
    alpha = float(params.get("hot_zipf_alpha", 1.15))
    period = int(params.get("period_events", 100_000))
    scan_n = int(params.get("scan_events_per_period", 20_000))
    scan_start = int(params.get("scan_start_key", 200_000))

    if hot_keyspace > ctx.keyspace:
        hot_keyspace = ctx.keyspace
    period = max(period, 1)
    scan_n = min(max(scan_n, 0), period)

    idx = np.arange(ctx.events, dtype=np.int64)
    pos = idx % period
    is_scan = pos < scan_n
    scan_keys = (scan_start + pos) % ctx.keyspace
    hot_keys = _zipf_finite(ctx.rng, ctx.events, hot_keyspace, alpha)
    keys = np.where(is_scan, scan_keys, hot_keys)
    labels = np.where(is_scan, "scan_cold", "stable_hot").astype(object)
    return keys, labels


def _gen_rotating_hot_sets(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    phase_len = int(params.get("phase_len_events", 100_000))
    phases = int(params.get("phases", 8))
    hotset_size = int(params.get("hotset_size", 50_000))
    hot_p = float(params.get("hot_probability", 0.9))

    phase_len = max(phase_len, 1)
    phases = max(phases, 1)
    hotset_size = max(hotset_size, 1)

    idx = np.arange(ctx.events, dtype=np.int64)
    phase_idx = (idx // phase_len) % phases
    starts = (phase_idx * hotset_size) % ctx.keyspace
    hot_off = ctx.rng.integers(0, hotset_size, size=ctx.events, dtype=np.int64)
    hot_keys = (starts + hot_off) % ctx.keyspace
    rand_keys = ctx.rng.integers(0, ctx.keyspace, size=ctx.events, dtype=np.int64)
    hot_mask = ctx.rng.random(ctx.events) < hot_p
    keys = np.where(hot_mask, hot_keys, rand_keys)

    phase_label_arr = np.array([f"phase_{i}" for i in range(phases)], dtype=object)
    hot_labels = phase_label_arr[phase_idx]
    labels = np.where(hot_mask, hot_labels, "background_random").astype(object)
    return keys, labels


def _gen_hot_core_noisy_tail(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    core_keyspace = int(params.get("core_keyspace", 10_000))
    alpha = float(params.get("core_zipf_alpha", 1.2))
    core_p = float(params.get("core_probability", 0.7))
    tail_start = int(params.get("tail_start_key", 100_000))

    if core_keyspace > ctx.keyspace:
        core_keyspace = ctx.keyspace

    core_keys = _zipf_finite(ctx.rng, ctx.events, core_keyspace, alpha)
    tail_idx = np.arange(ctx.events, dtype=np.int64)
    tail_keys = (tail_start + tail_idx) % ctx.keyspace
    core_mask = ctx.rng.random(ctx.events) < core_p
    keys = np.where(core_mask, core_keys, tail_keys)
    labels = np.where(core_mask, "stable_core", "noisy_tail").astype(object)
    return keys, labels


def _gen_size_skewed(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    alpha = float(params.get("zipf_alpha", 1.05))
    keys = _zipf_finite(ctx.rng, ctx.events, ctx.keyspace, alpha)
    labels = np.full(ctx.events, "size_skewed_zipf", dtype=object)
    return keys, labels


def _gen_read_churn_hot_region(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    refresh_every = int(params.get("refresh_every_events", 25_000))
    hotset_size = int(params.get("hotset_size", 25_000))
    hot_p = float(params.get("hot_probability", 0.85))

    refresh_every = max(refresh_every, 1)
    hotset_size = max(hotset_size, 1)

    n_refreshes = ctx.events // refresh_every + 1
    region_starts = ctx.rng.integers(0, ctx.keyspace, size=n_refreshes, dtype=np.int64)
    idx = np.arange(ctx.events, dtype=np.int64)
    starts_per_event = region_starts[idx // refresh_every]
    hot_off = ctx.rng.integers(0, hotset_size, size=ctx.events, dtype=np.int64)
    hot_keys = (starts_per_event + hot_off) % ctx.keyspace
    rand_keys = ctx.rng.integers(0, ctx.keyspace, size=ctx.events, dtype=np.int64)
    hot_mask = ctx.rng.random(ctx.events) < hot_p
    keys = np.where(hot_mask, hot_keys, rand_keys)
    labels = np.where(hot_mask, "churn_hot_region", "background_random").astype(object)
    return keys, labels


def _gen_sequential_scan(ctx: GenContext, params: dict) -> tuple[np.ndarray, np.ndarray]:
    """Sequential scan cycling through the keyspace. Worst case for LRU."""
    scan_size = int(params.get("scan_size", ctx.keyspace))
    keys = np.arange(ctx.events, dtype=np.int64) % scan_size
    labels = np.full(ctx.events, "sequential", dtype=object)
    return keys, labels


WORKLOADS: dict[str, Callable[[GenContext, dict], tuple[np.ndarray, np.ndarray]]] = {
    "uniform_random": _gen_uniform_random,
    "moving_hot_window": _gen_moving_hot_window,
    "stable_zipfian_hot_set": _gen_stable_zipfian_hot_set,
    "stable_hot_set_plus_scans": _gen_stable_hot_set_plus_scans,
    "rotating_hot_sets": _gen_rotating_hot_sets,
    "hot_core_noisy_tail": _gen_hot_core_noisy_tail,
    "size_skewed": _gen_size_skewed,
    "read_churn_hot_region": _gen_read_churn_hot_region,
    "sequential_scan": _gen_sequential_scan,
}


# ---------------------------------------------------------------------------
# Trace I/O
# ---------------------------------------------------------------------------

TRACE_COLUMNS = ["t", "op", "key", "key_size", "value_size", "workload_label"]


def _derive_workload_seed(global_seed: int, name: str) -> int:
    """Stable per-workload seed derived from global seed + workload name (FNV-1a-ish)."""
    MASK = (1 << 64) - 1
    PRIME = 0x100000001B3
    h = (int(global_seed) * PRIME) & MASK
    for c in name.encode("utf-8"):
        h = ((h ^ c) * PRIME) & MASK
    return int(h & 0x7FFFFFFF)


def generate_trace(
    name: str,
    cfg: HarnessConfig,
    progress: bool = True,
) -> pd.DataFrame:
    """Generate a single workload trace as a DataFrame."""
    if name not in WORKLOADS:
        raise KeyError(f"unknown workload: {name}")
    params = cfg.workloads.get(name, {}) or {}
    seed = _derive_workload_seed(cfg.global_.seed, name)
    rng = np.random.default_rng(seed)
    events = cfg.global_.events
    keyspace = cfg.global_.keyspace
    if progress:
        print(f"  [{name}] generating events={events} keyspace={keyspace} seed={seed}")
    ctx = GenContext(rng=rng, events=events, keyspace=keyspace)
    keys, labels = WORKLOADS[name](ctx, params)
    keys = np.asarray(keys, dtype=np.int64)
    sizes = value_sizes_for_keys(keys, cfg.value_sizes)
    # Compute key sizes from key:value ratio
    ratio_parts = cfg.value_sizes.key_value_ratio.split(":")
    k_ratio = int(ratio_parts[0])
    v_ratio = int(ratio_parts[1])
    key_sizes = np.maximum(1, (sizes * k_ratio // v_ratio)).astype(np.int64)
    t = np.arange(events, dtype=np.int64)
    op = np.full(events, "GET", dtype=object)
    df = pd.DataFrame(
        {
            "t": t,
            "op": op,
            "key": keys,
            "key_size": key_sizes,
            "value_size": sizes,
            "workload_label": labels,
        },
        columns=TRACE_COLUMNS,
    )
    return df


def generate_all_traces(
    cfg: HarnessConfig,
    out_dir: str | Path,
    progress: bool = True,
) -> list[Path]:
    """Generate every enabled workload trace and write CSVs to out_dir.

    Returns the list of written file paths.
    """
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    paths: list[Path] = []
    enabled = cfg.enabled_workloads
    if progress:
        print(f"Generating {len(enabled)} workload traces -> {out_dir}")
    for name in enabled:
        if name not in WORKLOADS:
            if progress:
                print(f"  [{name}] SKIP (unknown workload)")
            continue
        df = generate_trace(name, cfg, progress=progress)
        path = out_dir / f"{name}.csv"
        df.to_csv(path, index=False)
        paths.append(path)
        if progress:
            print(f"  [{name}] wrote {len(df)} rows -> {path}")
    return paths


def load_trace(path: str | Path) -> pd.DataFrame:
    """Load a trace CSV, validating the schema."""
    df = pd.read_csv(path)
    missing = [c for c in TRACE_COLUMNS if c not in df.columns]
    if missing:
        raise ValueError(f"trace file {path} missing columns: {missing}")
    return df


# ---------------------------------------------------------------------------
# Valkey MONITOR TRACE format (new CSV)
# ---------------------------------------------------------------------------

VALKEY_TRACE_COLUMNS = [
    "ts_us", "seq", "db_id", "cmd", "key", "access_type",
    "key_exists", "obj_type", "key_bytes", "value_bytes",
]


def load_valkey_trace(path: str | Path) -> pd.DataFrame:
    """Load a Valkey MONITOR TRACE CSV and convert to simulator format.

    The new format has columns:
        ts_us, seq, db_id, cmd, key (base64), access_type,
        key_exists, obj_type, key_bytes, value_bytes

    Returns a DataFrame with columns: key (int64), key_bytes (int64),
    value_bytes (int64), access_type (str), plus original columns.
    """
    path = Path(path)

    # Detect and skip non-header first line (e.g. "OK")
    with open(path) as f:
        first = f.readline().strip()
    skiprows = None
    if first and not any(c == ',' for c in first):
        skiprows = [0]

    df = pd.read_csv(
        path,
        header=None,
        skiprows=skiprows,
        names=VALKEY_TRACE_COLUMNS,
        dtype={
            "ts_us": np.int64,
            "seq": np.int64,
            "db_id": np.int32,
            "cmd": str,
            "key": str,
            "access_type": str,
            "key_exists": np.int32,
            "obj_type": str,
            "key_bytes": np.int64,
            "value_bytes": str,  # may be empty
        },
    )

    # value_bytes is empty when key doesn't exist — fill with 0
    df["value_bytes"] = pd.to_numeric(df["value_bytes"], errors="coerce").fillna(0).astype(np.int64)

    # Map base64 keys to dense integer IDs for the simulator
    unique_keys = df["key"].unique()
    key_map = {k: i for i, k in enumerate(unique_keys)}
    df["key_id"] = df["key"].map(key_map).astype(np.int64)

    return df
