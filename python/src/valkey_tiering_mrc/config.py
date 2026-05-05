"""Configuration loading and overrides for the MRC harness."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml


@dataclass
class GlobalConfig:
    seed: int = 42
    events: int = 1_000_000
    keyspace: int = 1_000_000
    chunk_size: int = 100_000
    capacity_points: int = 1001


@dataclass
class ValueSizesConfig:
    mode: str = "deterministic_heavy_tail"
    small_probability: float = 0.75
    medium_probability: float = 0.20
    large_probability: float = 0.05
    small_sizes: list[int] = field(default_factory=list)
    medium_sizes: list[int] = field(default_factory=list)
    large_sizes: list[int] = field(default_factory=list)
    key_value_ratio: str = "1:4"  # key_size = value_size / ratio_denominator


@dataclass
class HarnessConfig:
    """Top-level configuration object."""

    global_: GlobalConfig
    value_sizes: ValueSizesConfig
    workloads: dict[str, dict[str, Any]]

    @property
    def enabled_workloads(self) -> list[str]:
        return [
            name
            for name, params in self.workloads.items()
            if params.get("enabled", False)
        ]


def load_config(path: str | Path) -> HarnessConfig:
    """Load a HarnessConfig from a YAML file."""
    path = Path(path)
    with path.open("r", encoding="utf-8") as fh:
        data = yaml.safe_load(fh)
    return _from_dict(data)


def _from_dict(data: dict[str, Any]) -> HarnessConfig:
    g = data.get("global", {}) or {}
    vs = data.get("value_sizes", {}) or {}
    wl = data.get("workloads", {}) or {}
    return HarnessConfig(
        global_=GlobalConfig(
            seed=int(g.get("seed", 42)),
            events=int(g.get("events", 1_000_000)),
            keyspace=int(g.get("keyspace", 1_000_000)),
            chunk_size=int(g.get("chunk_size", 100_000)),
            capacity_points=int(g.get("capacity_points", 1001)),
        ),
        value_sizes=ValueSizesConfig(
            mode=str(vs.get("mode", "deterministic_heavy_tail")),
            small_probability=float(vs.get("small_probability", 0.75)),
            medium_probability=float(vs.get("medium_probability", 0.20)),
            large_probability=float(vs.get("large_probability", 0.05)),
            small_sizes=list(vs.get("small_sizes", [])),
            medium_sizes=list(vs.get("medium_sizes", [])),
            large_sizes=list(vs.get("large_sizes", [])),
            key_value_ratio=str(vs.get("key_value_ratio", "1:4")),
        ),
        workloads=dict(wl),
    )


def apply_overrides(
    cfg: HarnessConfig,
    *,
    events: int | None = None,
    keyspace: int | None = None,
    seed: int | None = None,
    capacity_points: int | None = None,
) -> HarnessConfig:
    """Return a new config with the given fields overridden when not None."""
    g = cfg.global_
    return HarnessConfig(
        global_=GlobalConfig(
            seed=int(seed) if seed is not None else g.seed,
            events=int(events) if events is not None else g.events,
            keyspace=int(keyspace) if keyspace is not None else g.keyspace,
            chunk_size=g.chunk_size,
            capacity_points=(
                int(capacity_points) if capacity_points is not None else g.capacity_points
            ),
        ),
        value_sizes=cfg.value_sizes,
        workloads=cfg.workloads,
    )
