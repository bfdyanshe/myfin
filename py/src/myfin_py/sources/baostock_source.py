"""Baostock adapter: financial master source for A-shares.

Covers daily bars (unadjusted, adjustflag=3), adj factors (back-adjust
cumulative factor), quarterly financials, earnings forecast/express reports
and the stock list. One connection is not thread-safe: every fetch opens a
bs.login()/bs.logout() pair and runs serially.
"""

from __future__ import annotations

import datetime as _dt
import time

import pandas as pd

from myfin_py.schema import (
    CANONICAL_FIELDS,
    canonicalize,
    empty_frame,
    normalize_symbol,
    to_date,
)

try:
    import baostock as bs
except Exception as exc:  # pragma: no cover - machine without SDK installed
    bs = None
    IMPORT_ERROR = f"baostock SDK import failed (pip install baostock): {exc}"
else:
    IMPORT_ERROR = None

from myfin_py.sources import BaseAdapter, SourceError  # noqa: E402

_BS_FIELDS = "date,open,high,low,close,volume,amount"
_WAN = 1e4  # baostock quarterly financials report amounts in 万元 -> 元


def to_bs_code(symbol: str) -> str:
    """Canonical `600519.SH` -> baostock `sh.600519` (unsupported exchanges raise)."""
    sym = normalize_symbol(symbol)
    code6, ex = sym.split(".")
    if ex not in ("SH", "SZ"):
        raise SourceError(
            f"baostock has no {ex} support (BSE/others excluded); got {symbol}",
            source="baostock",
        )
    return f"{ex.lower()}.{code6}"


def from_bs_code(code: str) -> str:
    """Baostock `sh.600519` -> canonical `600519.SH`."""
    prefix, code6 = code.split(".")
    return f"{code6}.{prefix.upper()}"


def _session():
    """Yield a logged-in baostock session; always logout on exit."""
    if bs is None:
        raise SourceError(IMPORT_ERROR, source="baostock")
    login = bs.login()
    if login.error_code != "0":
        raise SourceError(f"bs.login() failed: {login.error_msg}", source="baostock")
    try:
        yield bs
    finally:
        bs.logout()


def _rows(rs):
    if rs.error_code != "0":
        raise SourceError(f"baostock query failed: {rs.error_msg}", source="baostock")
    while rs.next():
        yield dict(zip(rs.fields, rs.get_row_data()))


def _num(rec, *names, default=None):
    for n in names:
        v = rec.get(n)
        if v not in (None, ""):
            try:
                return float(v)
            except ValueError:
                continue
    return default


class BaostockSource(BaseAdapter):
    name = "baostock"
    package = "myfin_py.sources.baostock_source"
    IMPORT_ERROR = IMPORT_ERROR
    DATASETS = ["daily", "adj_factor", "financial", "earnings_notice", "price_val"]
    PROBE_SYMBOL = "600519.SH"
    PROBE_LOOKBACK_DAYS = 5

    # ------------------------------------------------------------------
    # daily (unadjusted OHLCV)
    # ------------------------------------------------------------------

    def fetch_daily(self, symbol: str, start: str, end: str) -> pd.DataFrame:
        code = to_bs_code(symbol)
        with _session() as session:
            rs = session.query_history_k_data_plus(
                code, _BS_FIELDS, start_date=start, end_date=end,
                frequency="d", adjustflag="3",  # 3 = unadjusted
            )
            rows = list(_rows(rs))
        if not rows:
            return empty_frame("daily")
        df = pd.DataFrame(rows)
        df["trade_date"] = df["date"].map(to_date)
        for col in ("open", "high", "low", "close", "volume", "amount"):
            df[col] = pd.to_numeric(df[col], errors="coerce")
        # baostock volume unit is 股; unified schema stores 手 (100 股).
        df["volume"] = df["volume"] / 100.0
        df = df.dropna(subset=["close"])
        df["symbol"] = symbol
        df["source"] = self.name
        df = df[["trade_date", "open", "high", "low", "close", "volume", "amount", "symbol", "source"]]
        df = df.drop_duplicates(subset=["trade_date"]).sort_values("trade_date")
        return canonicalize(df, "daily")

    # ------------------------------------------------------------------
    # adj_factor (back-adjust cumulative factor)
    # ------------------------------------------------------------------

    def fetch_adj_factor(self, symbol: str) -> pd.DataFrame:
        code = to_bs_code(symbol)
        with _session() as session:
            rs = session.query_adjust_factor(code, start_date="", end_date="")
            rows = list(_rows(rs))
        if not rows:
            return empty_frame("adj_factor")
        df = pd.DataFrame(rows)
        df["ex_date"] = df["dividOperateDate"].map(to_date)
        # adjustflag=3 -> back-adjust cumulative factor; rows where it equals
        # 1 are kept (factor "1.0" is itself a valid ex-date marker).
        df["cum_factor"] = pd.to_numeric(df["backAdjustFactor"], errors="coerce")
        df = df.dropna(subset=["cum_factor"])
        df["symbol"] = symbol
        df["source"] = self.name
        df = df[["symbol", "ex_date", "cum_factor", "source"]]
        return canonicalize(df, "adj_factor")

    # ------------------------------------------------------------------
    # financial (quarterly snapshot; fields as list[(name, value)])
    # ------------------------------------------------------------------

    def fetch_financial(self, symbol: str) -> pd.DataFrame:
        code = to_bs_code(symbol)
        today = _dt.date.today()
        current_quarter = (today.month - 1) // 3 + 1
        snapshots = {}
        for year in range(2007, today.year + 1):
            max_quarter = current_quarter if year == today.year else 4
            for quarter in range(1, max_quarter + 1):
                for qf, mapper in (
                    (self._profit_data, self._map_profit),
                    (self._balance_data, self._map_balance),
                    (self._cash_flow_data, self._map_cash_flow),
                ):
                    with _session() as session:
                        rs = qf(session, code, year, quarter)
                        for rec in _rows(rs):
                            stat = rec.get("statDate")
                            if not stat:
                                continue
                            snap = snapshots.setdefault(stat, {})
                            snap.update(mapper(rec))
        if not snapshots:
            return empty_frame("financial")
        rows = []
        for stat, snap in sorted(snapshots.items()):
            fields = [(f, v) for f in CANONICAL_FIELDS if (v := snap.get(f)) is not None]
            if not fields:
                continue
            rows.append({
                "symbol": symbol,
                "report_period": to_date(stat),
                # Free source has no real announcement date: report end + 60d.
                "ann_date": to_date(stat) + _dt.timedelta(days=60),
                "fields": [{"name": f, "value": v} for f, v in fields],
                "source": self.name,
            })
        df = pd.DataFrame(rows)
        return canonicalize(df, "financial")

    def _profit_data(self, session, code, year, quarter):
        return session.query_profit_data(code=code, year=year, quarter=quarter)

    def _map_profit(self, rec):
        rev = _num(rec, "MBRevenue")
        np_ = _num(rec, "profit_net", "netProfit")  # yaml: net_profit from profit_net
        return {
            "revenue": rev * _WAN if rev else None,
            "net_profit": np_ * _WAN if np_ else None,
            "eps": _num(rec, "epsTTM"),  # no plain quarterly EPS; epsTTM as approximation
            "gross_margin": _num(rec, "gpMargin"),
            "roe": _num(rec, "roeAvg"),
        }

    def _balance_data(self, session, code, year, quarter):
        return session.query_balance_data(code=code, year=year, quarter=quarter)

    def _map_balance(self, rec):
        equity = _num(rec, "equity")
        return {
            "equity": equity * _WAN if equity else None,
            "debt_ratio": _num(rec, "liabilityToAsset"),
            # total_assets / total_liabilities are not exposed by baostock
            # free quarterly data; omitted from fields when unavailable.
        }

    def _cash_flow_data(self, session, code, year, quarter):
        return session.query_cash_flow_data(code=code, year=year, quarter=quarter)

    def _map_cash_flow(self, rec):
        # The free quarterly cash-flow API exposes only ratios (CFOToOR etc.),
        # not absolute operating cash flow; nothing to map.
        return {}

    # ------------------------------------------------------------------
    # earnings_notice (forecast + express)
    # ------------------------------------------------------------------

    def fetch_earnings_notice(self, symbol: str) -> pd.DataFrame:
        code = to_bs_code(symbol)
        start = "2003-01-01"
        end = time.strftime("%Y-%m-%d")
        rows = []
        with _session() as session:
            for rec in _rows(session.query_forecast_report(code, start_date=start, end_date=end)):
                rows.append(self._forecast_row(rec, symbol))
            for rec in _rows(session.query_performance_express_report(code, start_date=start, end_date=end)):
                rows.append(self._express_row(rec, symbol))
        if not rows:
            return empty_frame("earnings_notice")
        df = pd.DataFrame(rows)
        return canonicalize(df, "earnings_notice")

    def _forecast_row(self, rec, symbol):
        yoy = _mid(_num(rec, "profitYoy"), _num(rec, "profitYoyMax"))
        return {
            "symbol": symbol,
            "ann_date": to_date(rec.get("pubDate")) if rec.get("pubDate") else None,
            "report_period": to_date(rec.get("statDate")) if rec.get("statDate") else None,
            "kind": "forecast",
            # baostock forecast gives YoY + range only, no absolute profit.
            "net_profit": None,
            "net_profit_yoy": yoy,
            "source": self.name,
        }

    def _express_row(self, rec, symbol):
        np_ = _num(rec, "netProfit")
        return {
            "symbol": symbol,
            "ann_date": to_date(rec.get("pubDate")) if rec.get("pubDate") else None,
            "report_period": to_date(rec.get("statDate")) if rec.get("statDate") else None,
            "kind": "express",
            # express amounts are 万元 in baostock; convert to 元.
            "net_profit": np_ * _WAN if np_ else None,
            "net_profit_yoy": _num(rec, "profitYoY", "profitYoy"),
            "source": self.name,
        }


def _mid(lo, hi):
    if lo is None and hi is None:
        return None
    lo = lo if lo is not None else hi
    hi = hi if hi is not None else lo
    return (lo + hi) / 2.0
