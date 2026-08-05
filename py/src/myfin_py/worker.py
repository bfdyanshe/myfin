"""Worker CLI: fetch data from Python-SDK sources into staging Parquet.

IPC boundary with the Rust side (M3 orchestrator):
  - worker only writes Parquet to `--out` plus one manifest line per run
    (`<out>/manifest.jsonl`); Rust never calls into Python SDKs directly.
  - manifest line: {dataset, source, symbol, rows, status, updated_at}

Run from the repo root:
    PYTHONPATH=py/src python3 -m myfin_py.worker fetch-daily \\
        --source baostock --symbol 600519.SH --start 2021-01-01 --out data/staging
or after `uv sync` (editable install of py/):  python3 -m myfin_py.worker ...

Rate limits come from config/sources.toml (min_interval_ms per source);
a global per-source minimum interval is enforced in-process.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import os
import sys
import time
import tomllib
from pathlib import Path
from typing import Optional

import pandas as pd

from myfin_py.schema import to_arrow_table
from myfin_py.sources import REGISTRY, BaseAdapter, SourceError, get_source, list_sources

MANIFEST_NAME = "manifest.jsonl"
DATASETS = ("daily", "adj_factor", "financial", "earnings_notice")

# per-source minimum call interval in seconds (from config/sources.toml)
_last_call: dict[str, float] = {}


def load_rate_limits(registry_path: str) -> dict[str, float]:
    """Return {source_name: min_interval_seconds}; {} if unreadable."""
    try:
        with open(registry_path, "rb") as fh:
            data = tomllib.load(fh)
    except Exception:  # noqa: BLE001 - config problems must not block fetches
        return {}
    limits = {}
    for src in data.get("sources", []):
        rl = src.get("rate_limit") or {}
        limits[src["name"]] = rl.get("min_interval_ms", 0) / 1000.0
    return limits


def throttle(source: str, limits: dict[str, float]) -> None:
    interval = limits.get(source, 0.0)
    if interval <= 0:
        return
    now = time.monotonic()
    delta = now - _last_call.get(source, 0.0)
    if delta < interval:
        time.sleep(interval - delta)
    _last_call[source] = time.monotonic()


def write_parquet(df: pd.DataFrame, dataset: str, out: Path) -> Path:
    """Atomic write: <out>/<dataset>/<symbol>.parquet.tmp -> .parquet."""
    out.mkdir(parents=True, exist_ok=True)
    final = out / f"{dataset}.parquet"
    tmp = out / f"{dataset}.parquet.tmp"
    to_arrow_table(df, dataset).write_parquet(str(tmp))
    os.replace(tmp, final)
    return final


def append_manifest(out: Path, entry: dict) -> None:
    with open(out / MANIFEST_NAME, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(entry, ensure_ascii=False) + "\n")


def fetch_and_store(
    source: str,
    symbol: str,
    dataset: str,
    out: Path,
    limits: dict[str, float],
    start: Optional[str] = None,
    end: Optional[str] = None,
) -> int:
    adapter: BaseAdapter = get_source(source)
    throttle(source, limits)
    t0 = time.perf_counter()
    try:
        if dataset == "daily":
            df = adapter.fetch_daily(symbol, start=start, end=end)
        elif dataset == "adj_factor":
            df = adapter.fetch_adj_factor(symbol)
        elif dataset == "financial":
            df = adapter.fetch_financial(symbol)
        elif dataset == "earnings_notice":
            df = adapter.fetch_earnings_notice(symbol)
        else:
            raise SourceError(f"worker does not handle dataset {dataset!r}", source=source)
    except NotImplementedError as exc:
        entry = {"dataset": dataset, "source": source, "symbol": symbol,
                 "rows": 0, "status": "failed", "updated_at": _dt.datetime.now(_dt.timezone.utc).isoformat(),
                 "note": str(exc)}
        append_manifest(out, entry)
        print(f"FAIL {source}/{dataset}/{symbol}: {exc}", file=sys.stderr)
        return 1
    except SourceError as exc:
        entry = {"dataset": dataset, "source": source, "symbol": symbol,
                 "rows": 0, "status": "failed", "updated_at": _dt.datetime.now(_dt.timezone.utc).isoformat(),
                 "note": str(exc)}
        append_manifest(out, entry)
        print(f"FAIL {source}/{dataset}/{symbol}: {exc}", file=sys.stderr)
        return 1

    rows = len(df) if df is not None else 0
    if rows == 0:
        entry = {"dataset": dataset, "source": source, "symbol": symbol,
                 "rows": 0, "status": "skipped", "updated_at": _dt.datetime.now(_dt.timezone.utc).isoformat(),
                 "note": "no rows"}
        append_manifest(out, entry)
        print(f"SKIP {source}/{dataset}/{symbol}: no rows ({time.perf_counter() - t0:.2f}s)", file=sys.stderr)
        return 0

    final = write_parquet(df, dataset, out / dataset)
    entry = {"dataset": dataset, "source": source, "symbol": symbol,
             "rows": rows, "status": "done", "updated_at": _dt.datetime.now(_dt.timezone.utc).isoformat()}
    append_manifest(out, entry)
    print(f"OK   {source}/{dataset}/{symbol}: {rows} rows -> {final} ({time.perf_counter() - t0:.2f}s)")
    return 0


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        prog="myfin worker",
        description="Fetch data from Python-SDK sources into staging Parquet (see py/src/myfin_py/worker.py).",
    )
    parser.add_argument("--registry", default="config/sources.toml",
                        help="data-source registry path (rate limits; default config/sources.toml)")
    sub = parser.add_subparsers(dest="cmd", required=True)

    for dataset in DATASETS:
        p = sub.add_parser(f"fetch-{dataset}", help=f"fetch {dataset} for one symbol")
        p.add_argument("--source", required=True, help="source name (baostock/mootdx/akshare)")
        p.add_argument("--symbol", required=True, help="canonical symbol, e.g. 600519.SH")
        p.add_argument("--out", required=True, help="staging dir (manifest.jsonl is written here)")
        if dataset == "daily":
            p.add_argument("--start", default=None, help="YYYY-MM-DD (default: end - 5 years)")
            p.add_argument("--end", default=None, help="YYYY-MM-DD (default: today)")

    sub.add_parser("health-check", help="probe all registered sources")
    sub.add_parser("list-sources", help="list registered adapters")

    args = parser.parse_args(argv)
    limits = load_rate_limits(args.registry)

    if args.cmd == "list-sources":
        for s in list_sources():
            print(f"{s['name']:<12} {s['package']:<40} datasets: {','.join(s['datasets'])}")
        return 0

    if args.cmd == "health-check":
        failed = 0
        for name, cls in REGISTRY.items():
            rep = cls().health_check()
            mark = "OK " if rep["ok"] else "FAIL"
            if not rep["ok"]:
                failed += 1
            err = f" ({rep['error']})" if rep["error"] else ""
            print(f"{mark} {name:<12} latency={rep['latency_ms']}ms{err}")
        return 1 if failed else 0

    end = _dt.date.today().isoformat()
    start = ( _dt.date.today() - _dt.timedelta(days=5 * 365)).isoformat()
    if args.cmd == "fetch-daily":
        start = args.start or start
        end = args.end or end
    return fetch_and_store(args.source, args.symbol, args.cmd.removeprefix("fetch-"),
                           Path(args.out), limits, start=start, end=end)


if __name__ == "__main__":
    sys.exit(main())
