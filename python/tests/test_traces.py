"""Trace generation tests."""

from __future__ import annotations

from pathlib import Path

import pandas as pd

from valkey_tiering_mrc.config import load_config, apply_overrides
from valkey_tiering_mrc.traces import (
    TRACE_COLUMNS,
    WORKLOADS,
    generate_all_traces,
    value_sizes_for_keys,
)


def _config_path() -> Path:
    return Path(__file__).resolve().parent.parent / "examples" / "default_config.yaml"


def test_value_size_mapping_deterministic():
    cfg = load_config(_config_path())
    import numpy as np

    keys = np.arange(0, 10_000, dtype=np.int64)
    s1 = value_sizes_for_keys(keys, cfg.value_sizes)
    s2 = value_sizes_for_keys(keys, cfg.value_sizes)
    assert (s1 == s2).all()
    # Roughly matches small/medium/large probabilities
    small = set(cfg.value_sizes.small_sizes)
    medium = set(cfg.value_sizes.medium_sizes)
    large = set(cfg.value_sizes.large_sizes)
    n = keys.size
    p_small = sum(int(x) in small for x in s1) / n
    p_med = sum(int(x) in medium for x in s1) / n
    p_large = sum(int(x) in large for x in s1) / n
    assert 0.65 < p_small < 0.85
    assert 0.10 < p_med < 0.30
    assert 0.01 < p_large < 0.10


def test_generate_all_workloads(tmp_path: Path):
    cfg = load_config(_config_path())
    cfg = apply_overrides(cfg, events=100, keyspace=100, seed=7)
    out_dir = tmp_path / "traces"
    paths = generate_all_traces(cfg, out_dir, progress=False)

    enabled = cfg.enabled_workloads
    assert {p.stem for p in paths} == set(enabled)
    for p in paths:
        assert p.exists()
        df = pd.read_csv(p)
        assert list(df.columns) == TRACE_COLUMNS
        assert len(df) == 100
        assert (df["op"] == "GET").all()
        assert df["key"].between(0, 99).all()
        assert (df["value_size"] > 0).all()
        # workload_label must be non-empty strings
        assert df["workload_label"].notna().all()


def test_workload_registry_contains_eight():
    expected = {
        "uniform_random",
        "moving_hot_window",
        "stable_zipfian_hot_set",
        "stable_hot_set_plus_scans",
        "rotating_hot_sets",
        "hot_core_noisy_tail",
        "size_skewed",
        "read_churn_hot_region",
    }
    assert expected.issubset(WORKLOADS.keys())
