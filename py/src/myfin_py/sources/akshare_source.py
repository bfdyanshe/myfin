"""AKShare adapter: auxiliary source for macro indicators and earnings notices.

Macro endpoints are official (NBS/PBoC) and stable; Eastmoney endpoints
(stock_yjyg_em) have been unstable since 2025 anti-scraping upgrades and are
only a fallback. AKShare renames interfaces often - every call is wrapped in
try/except and column names are picked defensively (function names may change
between versions).
"""

from __future__ import annotations

import datetime as _dt
import time

import pandas as pd

from myfin_py.schema import canonicalize, empty_frame, normalize_symbol, to_date

try:
    import akshare as ak
except Exception as exc:  # pragma: no cover - machine without SDK installed
    ak = None
    IMPORT_ERROR = f"akshare SDK import failed (pip install akshare): {exc}"
else:
    IMPORT_ERROR = None

from myfin_py.sources import BaseAdapter, SourceError  # noqa: E402

# Official macro functions; candidates ordered by stability. If a name is
# renamed in a newer akshare release, add the new name to the list.
_MACRO_PROBES = [
    "macro_china_cpi_yearly",
    "macro_china_ppi_yearly",
    "macro_china_pmi_yearly",
]


class AKShareSource(BaseAdapter):
    name = "akshare"
    package = "myfin_py.sources.akshare_source"
    IMPORT_ERROR = IMPORT_ERROR
    DATASETS = ["macro", "earnings_notice"]

    # ------------------------------------------------------------------
    # health check: any working official macro endpoint suffices
    # ------------------------------------------------------------------

    def health_check(self) -> dict:
        if self.IMPORT_ERROR:
            return {"ok": False, "latency_ms": None, "error": self.IMPORT_ERROR}
        t0 = time.perf_counter()
        error = None
        for fn in _MACRO_PROBES:
            try:
                call = getattr(ak, fn)
                df = call()
                if df is not None and len(df) > 0:
                    return {"ok": True, "latency_ms": round((time.perf_counter() - t0) * 1000), "error": None}
            except Exception as exc:  # noqa: BLE001 - probe must not crash
                error = f"{fn}: {exc}"
        return {"ok": False, "latency_ms": round((time.perf_counter() - t0) * 1000), "error": error or "no macro probe succeeded"}

    # ------------------------------------------------------------------
    # earnings_notice (Eastmoney, unstable fallback)
    # ------------------------------------------------------------------

    def fetch_earnings_notice(self, symbol: str) -> pd.DataFrame:
        if self.IMPORT_ERROR:
            raise SourceError(self.IMPORT_ERROR, source=self.name)
        sym = normalize_symbol(symbol)
        rows = []
        # stock_yjyg_em is keyed by announcement date; probe the last 8
        # quarter-ends. Unstable interface: failures are skipped per date.
        for ann_date in _recent_quarter_ends(8):
            try:
                df = ak.stock_yjyg_em(date=ann_date.strftime("%Y%m%d"))
            except Exception as exc:  # noqa: BLE001 - Eastmoney is flaky by design
                raise SourceError(
                    f"akshare stock_yjyg_em failed for {ann_date}: {exc}",
                    source=self.name, operation="earnings_notice",
                ) from exc
            for _, rec in df.iterrows():
                code = str(rec.get("股票代码") or rec.get("代码") or "").strip()
                if not code or normalize_symbol(code) != sym:
                    continue
                rows.append(self._yjyg_row(rec, sym, ann_date))
        if not rows:
            return empty_frame("earnings_notice")
        return canonicalize(pd.DataFrame(rows), "earnings_notice")

    def _yjyg_row(self, rec, sym, fallback_ann):
        np_hi = _num(rec.get("预告净利润上限") or rec.get("预告净利润最大值"))
        np_lo = _num(rec.get("预告净利润下限") or rec.get("预告净利润最小值"))
        fields = {
            "symbol": sym,
            # 公告日期 may be absent for newer quarter probes -> fallback date
            "ann_date": _first_date(rec.get("公告日期") or rec.get("发布日期"), fallback_ann),
            "report_period": _first_date(rec.get("报告期") or rec.get("报告日期"), fallback_ann),
            "kind": "forecast",
            "net_profit": _mid(np_lo, np_hi),  # interval -> midpoint
            "net_profit_yoy": _num(rec.get("预告净利润变动幅度") or rec.get("净利润变动幅度") or rec.get("业绩变动幅度")),
            "source": self.name,
        }
        return fields


def _num(value):
    if value in (None, ""):
        return None
    try:
        v = float(value)
        return v if v == v else None  # reject NaN
    except (TypeError, ValueError):
        return None


def _mid(lo, hi):
    if lo is None and hi is None:
        return None
    lo = lo if lo is not None else hi
    hi = hi if hi is not None else lo
    return (lo + hi) / 2.0


def _first_date(value, fallback):
    if value in (None, ""):
        return fallback
    try:
        return to_date(str(value))
    except (ValueError, TypeError):
        return fallback


def _recent_quarter_ends(n: int):
    """Last `n` quarter-end dates, newest first."""
    year, month = _dt.date.today().year, _dt.date.today().month
    ends = []
    for _ in range(n):
        quarter = (month - 1) // 3
        ends.append(_dt.date(year, [3, 6, 9, 12][quarter], [31, 30, 30, 31][quarter]))
        month -= 3
        if month <= 0:
            month += 12
            year -= 1
    return ends
