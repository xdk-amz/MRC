"""Plotting utilities: per-workload MRC charts and contact sheets."""

from __future__ import annotations

import math
from pathlib import Path

import matplotlib

matplotlib.use("Agg")  # non-interactive backend for headless runs
import matplotlib.pyplot as plt  # noqa: E402
import pandas as pd  # noqa: E402
from PIL import Image  # noqa: E402


def _set_full_axes(ax) -> None:
    ax.set_xlim(0, 100)
    ax.set_ylim(0, 100)
    ax.grid(True, linestyle="--", alpha=0.4)


def _mode_title(df: pd.DataFrame) -> str:
    mode = str(df.get("measurement_mode", pd.Series(["unknown"])) .iloc[0]) if len(df) else "unknown"
    if mode == "exclude_first_touch":
        return "First-touch-excluded"
    if mode == "cyclic":
        return "Cyclic"
    return mode


def plot_forward_mrc(
    df: pd.DataFrame, workload: str, out_path: Path
) -> Path:
    """Plot forward MRC for a single workload (object miss + byte miss vs capacity)."""
    sub = df[df["workload"] == workload].sort_values("capacity_fraction_of_unique_bytes")
    fig, ax = plt.subplots(figsize=(6, 4.5), dpi=120)
    cap_pct = sub["capacity_fraction_of_unique_bytes"].to_numpy() * 100.0
    obj = sub["object_miss_ratio"].to_numpy() * 100.0
    byt = sub["byte_miss_ratio"].to_numpy() * 100.0
    ax.plot(cap_pct, obj, label="object/request miss", linewidth=2, color="#1f77b4")
    ax.plot(cap_pct, byt, label="byte miss", linewidth=2, color="#d62728")
    ax.set_xlabel("DRAM capacity (% of unique value bytes)")
    ax.set_ylabel("miss ratio (%)")
    ax.set_title(f"{_mode_title(sub)} {str(sub['policy'].iloc[0]).split('_')[0].upper()} MRC — {workload}")
    ax.legend(loc="upper right")
    _set_full_axes(ax)
    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def plot_inverse_mrc(
    df: pd.DataFrame, workload: str, out_path: Path
) -> Path:
    """Plot inverse MRC for a single workload."""
    sub = df[df["workload"] == workload].sort_values("target_miss_ratio_percent")
    fig, ax = plt.subplots(figsize=(6, 4.5), dpi=120)
    targets = sub["target_miss_ratio_percent"].to_numpy()
    req_obj = sub["required_capacity_percent_of_unique_bytes_for_request_miss"].to_numpy()
    req_byte = sub["required_capacity_percent_of_unique_bytes_for_byte_miss"].to_numpy()
    ax.plot(targets, req_obj, label="request/object miss target", linewidth=2, color="#1f77b4")
    ax.plot(targets, req_byte, label="byte miss target", linewidth=2, color="#d62728")
    ax.set_xlabel("target miss ratio (%)")
    ax.set_ylabel("required DRAM capacity (% of unique value bytes)")
    ax.set_title(f"{_mode_title(sub)} inverse MRC — {workload}")
    ax.legend(loc="upper right")
    _set_full_axes(ax)
    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def make_contact_sheet(image_paths: list[Path], out_path: Path, cols: int = 3) -> Path:
    """Combine PNGs into a single grid image (contact sheet)."""
    if not image_paths:
        raise ValueError("no images to assemble")
    cols = max(1, int(cols))
    images = [Image.open(p).convert("RGB") for p in image_paths]
    w = max(im.width for im in images)
    h = max(im.height for im in images)
    rows = math.ceil(len(images) / cols)
    sheet = Image.new("RGB", (w * cols, h * rows), color="white")
    for idx, im in enumerate(images):
        r, c = divmod(idx, cols)
        sheet.paste(im, (c * w, r * h))
    out_path.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(out_path)
    for im in images:
        im.close()
    return out_path


def plot_all(
    forward_csv: Path,
    inverse_csv: Path,
    out_dir: Path,
    cols: int = 3,
) -> dict:
    """Generate per-workload forward/inverse plots plus contact sheets.

    Returns dict with keys: forward_plots, inverse_plots, forward_sheet, inverse_sheet.
    """
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    fwd = pd.read_csv(forward_csv)
    inv = pd.read_csv(inverse_csv)

    workloads = sorted(fwd["workload"].unique().tolist())

    forward_plots: list[Path] = []
    inverse_plots: list[Path] = []
    for wl in workloads:
        fp = out_dir / f"forward_{wl}.png"
        ip = out_dir / f"inverse_{wl}.png"
        plot_forward_mrc(fwd, wl, fp)
        plot_inverse_mrc(inv, wl, ip)
        forward_plots.append(fp)
        inverse_plots.append(ip)

    fwd_sheet = make_contact_sheet(forward_plots, out_dir / "forward_contact_sheet.png", cols=cols)
    inv_sheet = make_contact_sheet(inverse_plots, out_dir / "inverse_contact_sheet.png", cols=cols)

    return {
        "forward_plots": forward_plots,
        "inverse_plots": inverse_plots,
        "forward_sheet": fwd_sheet,
        "inverse_sheet": inv_sheet,
    }


# ---------------------------------------------------------------------------
# Policy comparison plots (e.g. LRU vs true LFU).
# ---------------------------------------------------------------------------

LRU_COLOR = "#1f77b4"
LFU_COLOR = "#2ca02c"
OBJECT_LS = "-"
BYTE_LS = "--"


def _sorted_forward(df: pd.DataFrame, workload: str) -> pd.DataFrame:
    return df[df["workload"] == workload].sort_values(
        "capacity_fraction_of_unique_bytes"
    )


def _sorted_inverse(df: pd.DataFrame, workload: str) -> pd.DataFrame:
    return df[df["workload"] == workload].sort_values("target_miss_ratio_percent")


def plot_compare_forward_object(
    lru_df: pd.DataFrame,
    lfu_df: pd.DataFrame,
    workload: str,
    out_path: Path,
) -> Path:
    """Two-line forward chart (object/request miss): LRU vs true LFU."""
    fig, ax = plt.subplots(figsize=(6, 4.5), dpi=120)
    for df, label, color in (
        (lru_df, "LRU", LRU_COLOR),
        (lfu_df, "true LFU", LFU_COLOR),
    ):
        sub = _sorted_forward(df, workload)
        cap = sub["capacity_fraction_of_unique_bytes"].to_numpy() * 100.0
        miss = sub["object_miss_ratio"].to_numpy() * 100.0
        ax.plot(cap, miss, label=label, linewidth=2, color=color)
    ax.set_xlabel("DRAM capacity (% of unique value bytes)")
    ax.set_ylabel("object/request miss ratio (%)")
    ax.set_title(f"LRU vs true LFU (object miss) — {workload}")
    ax.legend(loc="upper right")
    _set_full_axes(ax)
    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def plot_compare_forward_byte(
    lru_df: pd.DataFrame,
    lfu_df: pd.DataFrame,
    workload: str,
    out_path: Path,
) -> Path:
    """Two-line forward chart (byte miss): LRU vs true LFU."""
    fig, ax = plt.subplots(figsize=(6, 4.5), dpi=120)
    for df, label, color in (
        (lru_df, "LRU", LRU_COLOR),
        (lfu_df, "true LFU", LFU_COLOR),
    ):
        sub = _sorted_forward(df, workload)
        cap = sub["capacity_fraction_of_unique_bytes"].to_numpy() * 100.0
        miss = sub["byte_miss_ratio"].to_numpy() * 100.0
        ax.plot(cap, miss, label=label, linewidth=2, color=color)
    ax.set_xlabel("DRAM capacity (% of unique value bytes)")
    ax.set_ylabel("byte miss ratio (%)")
    ax.set_title(f"LRU vs true LFU (byte miss) — {workload}")
    ax.legend(loc="upper right")
    _set_full_axes(ax)
    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def plot_compare_forward_combined(
    lru_df: pd.DataFrame,
    lfu_df: pd.DataFrame,
    workload: str,
    out_path: Path,
) -> Path:
    """Four-line forward chart: LRU object, LRU byte, LFU object, LFU byte."""
    fig, ax = plt.subplots(figsize=(6, 4.5), dpi=120)
    for df, policy_label, color in (
        (lru_df, "LRU", LRU_COLOR),
        (lfu_df, "true LFU", LFU_COLOR),
    ):
        sub = _sorted_forward(df, workload)
        cap = sub["capacity_fraction_of_unique_bytes"].to_numpy() * 100.0
        obj = sub["object_miss_ratio"].to_numpy() * 100.0
        byt = sub["byte_miss_ratio"].to_numpy() * 100.0
        ax.plot(cap, obj, label=f"{policy_label} object", linewidth=2,
                color=color, linestyle=OBJECT_LS)
        ax.plot(cap, byt, label=f"{policy_label} byte", linewidth=2,
                color=color, linestyle=BYTE_LS)
    ax.set_xlabel("DRAM capacity (% of unique value bytes)")
    ax.set_ylabel("miss ratio (%)")
    ax.set_title(f"LRU vs true LFU — {workload}")
    ax.legend(loc="upper right", fontsize=8)
    _set_full_axes(ax)
    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def plot_compare_inverse_object(
    lru_df: pd.DataFrame,
    lfu_df: pd.DataFrame,
    workload: str,
    out_path: Path,
) -> Path:
    """LRU vs true LFU required capacity for request/object miss target."""
    fig, ax = plt.subplots(figsize=(6, 4.5), dpi=120)
    for df, label, color in (
        (lru_df, "LRU", LRU_COLOR),
        (lfu_df, "true LFU", LFU_COLOR),
    ):
        sub = _sorted_inverse(df, workload)
        x = sub["target_miss_ratio_percent"].to_numpy()
        y = sub[
            "required_capacity_percent_of_unique_bytes_for_request_miss"
        ].to_numpy()
        ax.plot(x, y, label=label, linewidth=2, color=color)
    ax.set_xlabel("target object/request miss ratio (%)")
    ax.set_ylabel("required DRAM capacity (% of unique value bytes)")
    ax.set_title(f"LRU vs true LFU (object inverse) — {workload}")
    ax.legend(loc="upper right")
    _set_full_axes(ax)
    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def plot_compare_inverse_byte(
    lru_df: pd.DataFrame,
    lfu_df: pd.DataFrame,
    workload: str,
    out_path: Path,
) -> Path:
    """LRU vs true LFU required capacity for byte miss target."""
    fig, ax = plt.subplots(figsize=(6, 4.5), dpi=120)
    for df, label, color in (
        (lru_df, "LRU", LRU_COLOR),
        (lfu_df, "true LFU", LFU_COLOR),
    ):
        sub = _sorted_inverse(df, workload)
        x = sub["target_miss_ratio_percent"].to_numpy()
        y = sub[
            "required_capacity_percent_of_unique_bytes_for_byte_miss"
        ].to_numpy()
        ax.plot(x, y, label=label, linewidth=2, color=color)
    ax.set_xlabel("target byte miss ratio (%)")
    ax.set_ylabel("required DRAM capacity (% of unique value bytes)")
    ax.set_title(f"LRU vs true LFU (byte inverse) — {workload}")
    ax.legend(loc="upper right")
    _set_full_axes(ax)
    fig.tight_layout()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def plot_policy_inverse_sheet(
    inverse_csv: Path,
    out_dir: Path,
    sheet_name: str,
    cols: int = 3,
) -> Path:
    """Render per-workload inverse plots from `inverse_csv` and assemble a sheet."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    inv = pd.read_csv(inverse_csv)
    workloads = sorted(inv["workload"].unique().tolist())
    plots: list[Path] = []
    for wl in workloads:
        p = out_dir / f"inverse_{wl}.png"
        plot_inverse_mrc(inv, wl, p)
        plots.append(p)
    return make_contact_sheet(plots, out_dir / sheet_name, cols=cols)


def plot_compare_policies(
    lru_csv: Path,
    lfu_csv: Path,
    out_dir: Path,
    lru_inverse_csv: Path | None = None,
    lfu_inverse_csv: Path | None = None,
    cols: int = 3,
) -> dict:
    """Generate per-workload comparison charts plus contact sheets.

    Forward charts (always):
      - object miss: lru_vs_true_lfu_object_<workload>.png
      - byte miss:   lru_vs_true_lfu_byte_<workload>.png
      - combined:    lru_vs_true_lfu_combined_<workload>.png

    Inverse charts (only when both inverse CSVs are provided):
      - object: lru_vs_true_lfu_inverse_object_<workload>.png
      - byte:   lru_vs_true_lfu_inverse_byte_<workload>.png

    Plus contact sheets:
      - lru_vs_true_lfu_object_contact_sheet.png
      - lru_vs_true_lfu_byte_contact_sheet.png
      - lru_vs_true_lfu_combined_contact_sheet.png
      - lru_vs_true_lfu_inverse_object_contact_sheet.png  (if inverses given)
      - lru_vs_true_lfu_inverse_byte_contact_sheet.png    (if inverses given)
    """
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    lru_df = pd.read_csv(lru_csv)
    lfu_df = pd.read_csv(lfu_csv)

    workloads = sorted(set(lru_df["workload"]).intersection(lfu_df["workload"]))
    obj_plots: list[Path] = []
    byte_plots: list[Path] = []
    combined_plots: list[Path] = []
    inv_obj_plots: list[Path] = []
    inv_byte_plots: list[Path] = []

    inv_lru_df = pd.read_csv(lru_inverse_csv) if lru_inverse_csv else None
    inv_lfu_df = pd.read_csv(lfu_inverse_csv) if lfu_inverse_csv else None

    for wl in workloads:
        obj_plots.append(
            plot_compare_forward_object(
                lru_df, lfu_df, wl, out_dir / f"lru_vs_true_lfu_object_{wl}.png"
            )
        )
        byte_plots.append(
            plot_compare_forward_byte(
                lru_df, lfu_df, wl, out_dir / f"lru_vs_true_lfu_byte_{wl}.png"
            )
        )
        combined_plots.append(
            plot_compare_forward_combined(
                lru_df, lfu_df, wl, out_dir / f"lru_vs_true_lfu_combined_{wl}.png"
            )
        )
        if inv_lru_df is not None and inv_lfu_df is not None:
            inv_obj_plots.append(
                plot_compare_inverse_object(
                    inv_lru_df, inv_lfu_df, wl,
                    out_dir / f"lru_vs_true_lfu_inverse_object_{wl}.png",
                )
            )
            inv_byte_plots.append(
                plot_compare_inverse_byte(
                    inv_lru_df, inv_lfu_df, wl,
                    out_dir / f"lru_vs_true_lfu_inverse_byte_{wl}.png",
                )
            )

    sheets = {
        "object_sheet": make_contact_sheet(
            obj_plots, out_dir / "lru_vs_true_lfu_object_contact_sheet.png", cols=cols
        ),
        "byte_sheet": make_contact_sheet(
            byte_plots, out_dir / "lru_vs_true_lfu_byte_contact_sheet.png", cols=cols
        ),
        "combined_sheet": make_contact_sheet(
            combined_plots,
            out_dir / "lru_vs_true_lfu_combined_contact_sheet.png",
            cols=cols,
        ),
    }
    if inv_obj_plots:
        sheets["inverse_object_sheet"] = make_contact_sheet(
            inv_obj_plots,
            out_dir / "lru_vs_true_lfu_inverse_object_contact_sheet.png",
            cols=cols,
        )
    if inv_byte_plots:
        sheets["inverse_byte_sheet"] = make_contact_sheet(
            inv_byte_plots,
            out_dir / "lru_vs_true_lfu_inverse_byte_contact_sheet.png",
            cols=cols,
        )

    return {
        "object_plots": obj_plots,
        "byte_plots": byte_plots,
        "combined_plots": combined_plots,
        "inverse_object_plots": inv_obj_plots,
        "inverse_byte_plots": inv_byte_plots,
        **sheets,
    }
