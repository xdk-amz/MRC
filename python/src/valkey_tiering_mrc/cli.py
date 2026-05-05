"""Command-line interface: trace generation and plotting."""

from __future__ import annotations

import argparse
from pathlib import Path

from . import plot, traces
from .config import apply_overrides, load_config


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="valkey-tiering-mrc",
        description="Trace generation and MRC plotting tools.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    # generate-traces
    p_gen = sub.add_parser("generate-traces", help="Generate synthetic GET-only traces.")
    p_gen.add_argument("--config", required=True, type=Path)
    p_gen.add_argument("--out", required=True, type=Path)
    p_gen.add_argument("--events", type=int, default=None)
    p_gen.add_argument("--keyspace", type=int, default=None)
    p_gen.add_argument("--seed", type=int, default=None)

    # plot
    p_plot = sub.add_parser("plot", help="Generate charts from MRC CSV files.")
    p_plot.add_argument("--curves", required=True, type=Path, help="Forward MRC CSV.")
    p_plot.add_argument("--inverse", required=True, type=Path, help="Inverse MRC CSV.")
    p_plot.add_argument("--out", required=True, type=Path)
    p_plot.add_argument("--cols", type=int, default=3)

    return parser


def cmd_generate_traces(args: argparse.Namespace) -> int:
    cfg = load_config(args.config)
    cfg = apply_overrides(cfg, events=args.events, keyspace=args.keyspace, seed=args.seed)
    traces.generate_all_traces(cfg, args.out, progress=True)
    return 0


def cmd_plot(args: argparse.Namespace) -> int:
    plot.plot_all(
        forward_csv=Path(args.curves),
        inverse_csv=Path(args.inverse),
        out_dir=Path(args.out),
        cols=args.cols,
    )
    return 0


COMMANDS = {
    "generate-traces": cmd_generate_traces,
    "plot": cmd_plot,
}


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    return COMMANDS[args.cmd](args)
