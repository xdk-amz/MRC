"""Pipeline: trace generation orchestration."""

from __future__ import annotations

from pathlib import Path

from . import traces
from .config import HarnessConfig


def generate_traces(cfg: HarnessConfig, out_dir: Path, progress: bool = True) -> list[Path]:
    """Generate synthetic traces from config."""
    return traces.generate_all_traces(cfg, out_dir, progress=progress)
