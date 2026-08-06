"""Source registry: name -> adapter class, plus the shared base adapter.

Each adapter module guards its third-party SDK import with try/except and
exposes `IMPORT_ERROR`, so the registry imports cleanly even when SDKs are
not installed (health_check reports the missing dependency instead).
"""

from __future__ import annotations

import datetime as _dt
import importlib
import time
from typing import Optional


class SourceError(RuntimeError):
    """Structured, user-visible failure (network, auth, rate limit, bad data)."""

    def __init__(self, message: str, *, source: Optional[str] = None, operation: Optional[str] = None):
        super().__init__(message)
        self.source = source
        self.operation = operation


class BaseAdapter:
    """Common contract for all Python data-source adapters.

    `capabilities()` follows config/sources.toml. Any fetch method a source
    does not support raises NotImplementedError (the Rust side never calls
    it, since it only schedules datasets the source advertises).
    """

    name: str = "base"
    package: str = "myfin_py.sources"
    IMPORT_ERROR: Optional[str] = None
    DATASETS: list = []
    PROBE_SYMBOL: str = ""
    PROBE_LOOKBACK_DAYS: int = 5

    def capabilities(self) -> list:
        return list(self.DATASETS)

    def health_check(self) -> dict:
        """Probe the source; returns {ok, latency_ms, error}."""
        if self.IMPORT_ERROR:
            return {"ok": False, "latency_ms": None, "error": self.IMPORT_ERROR}
        end = _dt.date.today()
        start = end - _dt.timedelta(days=self.PROBE_LOOKBACK_DAYS)
        t0 = time.perf_counter()
        try:
            df = self.fetch_daily(self.PROBE_SYMBOL, start=start.isoformat(), end=end.isoformat())
            ok = df is not None and len(df) > 0
            error = None if ok else "probe returned no rows"
        except Exception as exc:  # noqa: BLE001 - probe must never crash the orchestrator
            ok, error = False, str(exc)
        return {"ok": ok, "latency_ms": round((time.perf_counter() - t0) * 1000), "error": error}

    # ------------------------------------------------------------------
    # Fetch methods (canonical DataFrames, see schema.py)
    # ------------------------------------------------------------------

    def fetch_daily(self, symbol: str, start: str, end: str):
        raise NotImplementedError(f"{self.name}: daily not implemented")

    def fetch_adj_factor(self, symbol: str):
        raise NotImplementedError(f"{self.name}: adj_factor not implemented")

    def fetch_financial(self, symbol: str, ann_date_approx_days: int = 60):
        raise NotImplementedError(f"{self.name}: financial not implemented")

    def fetch_earnings_notice(self, symbol: str):
        raise NotImplementedError(f"{self.name}: earnings_notice not implemented")

    def fetch_price_val(self, symbol: str):
        raise NotImplementedError(f"{self.name}: price_val not implemented")

    def fetch_macro(self, start: str, end: str):
        raise NotImplementedError(f"{self.name}: macro not implemented")


_ADAPTERS = {
    "baostock": ("myfin_py.sources.baostock_source", "BaostockSource"),
    "akshare": ("myfin_py.sources.akshare_source", "AKShareSource"),
    "mootdx": ("myfin_py.sources.mootdx_source", "MootdxSource"),
}

REGISTRY: dict = {}
IMPORT_ERRORS: dict[str, str] = {}
for _name, (_module_name, _class_name) in _ADAPTERS.items():
    try:
        _module = importlib.import_module(_module_name)
        REGISTRY[_name] = getattr(_module, _class_name)
    except Exception as exc:  # noqa: BLE001 - health check reports dependency failures
        IMPORT_ERRORS[_name] = f"Python adapter import failed: {exc}"


def get_source(name: str) -> BaseAdapter:
    cls = REGISTRY.get(name)
    if cls is None:
        if name in IMPORT_ERRORS:
            raise SourceError(IMPORT_ERRORS[name], source=name)
        raise SourceError(f"unknown source {name!r}; registered: {sorted(REGISTRY)}")
    return cls()


def list_sources() -> list:
    sources = [
        {"name": name, "package": cls.package, "datasets": cls().capabilities()}
        for name, cls in REGISTRY.items()
    ]
    sources.extend(
        {"name": name, "package": module, "datasets": [], "error": error}
        for name, error in IMPORT_ERRORS.items()
        for module, _class_name in [_ADAPTERS[name]]
    )
    return sources
