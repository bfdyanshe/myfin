"""Baostock adapter: financial master source for A-shares.

Covers daily bars (unadjusted, adjustflag=3), adj factors (back-adjust
cumulative factor), quarterly financials, earnings forecast/express reports
and the stock list. One connection is not thread-safe: every fetch opens a
bs.login()/bs.logout() pair and runs serially.
"""

from __future__ import annotations

import datetime as _dt
from functools import wraps
import os
import time
from bisect import bisect_right
from contextlib import contextmanager

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
    from baostock.common import contants as bs_constants
except Exception as exc:  # pragma: no cover - machine without SDK installed
    bs = None
    bs_constants = None
    IMPORT_ERROR = f"baostock SDK import failed (pip install baostock): {exc}"
else:
    IMPORT_ERROR = None

from myfin_py.sources import BaseAdapter, SourceError  # noqa: E402

_BS_FIELDS = "date,open,high,low,close,volume,amount"
_BLACKLIST_COOLDOWN_SECONDS = 3600.0
_QUERY_INTERVAL_SECONDS = 0.8
_blacklisted_until = 0.0

_UNAVAILABLE_ERROR_CODES = {
    "BSERR_NO_LOGIN": "10001001",
    "BSERR_LOGIN_COUNT_LIMIT": "10001005",
    "BSERR_BLACKLIST_USER": "10001011",
    "BSERR_SOCKET_ERR": "10002001",
    "BSERR_CONNECT_FAIL": "10002002",
    "BSERR_CONNECT_TIMEOUT": "10002003",
    "BSERR_RECVCONNECTION_CLOSED": "10002004",
    "BSERR_SENDSOCK_FAIL": "10002005",
    "BSERR_SENDSOCK_TIMEOUT": "10002006",
    "BSERR_RECVSOCK_FAIL": "10002007",
    "BSERR_RECVSOCK_TIMEOUT": "10002008",
}


class _BaostockUnavailable(SourceError):
    """Baostock 服务或连接暂不可用，可按空结果继续流水线。"""


def _is_unavailable_code(error_code: str) -> bool:
    return error_code in {
        getattr(bs_constants, name, fallback) if bs_constants is not None else fallback
        for name, fallback in _UNAVAILABLE_ERROR_CODES.items()
    }


def _empty_on_unavailable(dataset):
    def decorate(function):
        @wraps(function)
        def wrapped(self, *args, **kwargs):
            try:
                return function(self, *args, **kwargs)
            except _BaostockUnavailable as exc:
                self._last_unavailable_error = str(exc)
                return empty_frame(dataset)

        return wrapped

    return decorate


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


@contextmanager
def _session():
    """Yield a logged-in baostock session; always logout on exit."""
    global _blacklisted_until
    if bs is None:
        raise SourceError(IMPORT_ERROR, source="baostock")
    remaining = _blacklisted_until - time.monotonic()
    if remaining > 0:
        raise _BaostockUnavailable(
            "Baostock 服务端拒绝当前访问来源（黑名单用户），"
            f"本进程将在 {remaining:.0f} 秒内停止重试；本次数据按空结果处理",
            source="baostock",
            operation="login",
        )
    try:
        login = bs.login(
            user_id=os.environ.get("BAOSTOCK_USER", "anonymous"),
            password=os.environ.get("BAOSTOCK_PASSWORD", "123456"),
        )
    except Exception as exc:  # noqa: BLE001 - SDK turns network failures into varied exceptions
        raise _BaostockUnavailable(
            f"Baostock 登录连接异常：{exc}；本次数据按空结果处理",
            source="baostock",
            operation="login",
        ) from exc
    if login.error_code != "0":
        if _is_unavailable_code(login.error_code) or "黑名单" in (login.error_msg or ""):
            blacklist_code = getattr(bs_constants, "BSERR_BLACKLIST_USER", "10001011")
            if login.error_code == blacklist_code or "黑名单" in (login.error_msg or ""):
                _blacklisted_until = time.monotonic() + _BLACKLIST_COOLDOWN_SECONDS
            raise _BaostockUnavailable(
                f"Baostock 登录不可用（错误码 {login.error_code}：{login.error_msg}），"
                "本次数据按空结果处理",
                source="baostock",
                operation="login",
            )
        raise SourceError(
            f"bs.login() failed ({login.error_code}): {login.error_msg}",
            source="baostock",
            operation="login",
        )
    try:
        yield bs
    finally:
        bs.logout()


class _QueryPacer:
    def __init__(self, interval: float = _QUERY_INTERVAL_SECONDS):
        self._interval = interval
        self._last_call = 0.0

    def call(self, function, *args, **kwargs):
        delay = self._interval - (time.monotonic() - self._last_call)
        if delay > 0:
            time.sleep(delay)
        result = function(*args, **kwargs)
        self._last_call = time.monotonic()
        return result


def _rows(rs):
    if rs.error_code != "0":
        if _is_unavailable_code(rs.error_code):
            raise _BaostockUnavailable(
                f"Baostock 查询连接不可用（错误码 {rs.error_code}：{rs.error_msg}），"
                "本次数据按空结果处理",
                source="baostock",
                operation="query",
            )
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

    def health_check(self) -> dict:
        self._last_unavailable_error = None
        report = super().health_check()
        if self._last_unavailable_error:
            report["error"] = self._last_unavailable_error
        return report

    # ------------------------------------------------------------------
    # daily (unadjusted OHLCV)
    # ------------------------------------------------------------------

    @_empty_on_unavailable("daily")
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

    @_empty_on_unavailable("adj_factor")
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

    @_empty_on_unavailable("financial")
    def fetch_financial(self, symbol: str, ann_date_approx_days: int = 60) -> pd.DataFrame:
        code = to_bs_code(symbol)
        today = _dt.date.today()
        current_quarter = (today.month - 1) // 3 + 1
        snapshots = {}
        pacer = _QueryPacer()
        with _session() as session:
            for year in range(2007, today.year + 1):
                max_quarter = current_quarter if year == today.year else 4
                for quarter in range(1, max_quarter + 1):
                    for qf, mapper in (
                        (self._profit_data, self._map_profit),
                        (self._balance_data, self._map_balance),
                        (self._cash_flow_data, self._map_cash_flow),
                    ):
                        rs = pacer.call(qf, session, code, year, quarter)
                        for rec in _rows(rs):
                            stat = rec.get("statDate")
                            if not stat:
                                continue
                            snap = snapshots.setdefault(
                                stat, {"fields": {}, "raw_fields": {}, "pub_dates": []}
                            )
                            mapped = mapper(rec)
                            snap["fields"].update(mapped)
                            snap["raw_fields"].update(mapped)
                            pub_date = _parse_date(rec.get("pubDate"))
                            if pub_date is not None:
                                snap["pub_dates"].append(pub_date)
        if not snapshots:
            return empty_frame("financial")
        rows = []
        previous_ytd = {}
        for stat, snap in sorted(snapshots.items()):
            report_period = to_date(stat)
            raw_fields = snap["raw_fields"]
            year = report_period.year
            quarter = (report_period.month - 1) // 3 + 1
            fields_map = dict(raw_fields)
            if quarter > 1:
                prior = previous_ytd.get(year, {})
                for field in ("revenue", "net_profit"):
                    if fields_map.get(field) is not None and prior.get(field) is not None:
                        fields_map[field] -= prior[field]
            previous_ytd[year] = dict(raw_fields)
            fields = [
                (field, value)
                for field in CANONICAL_FIELDS
                if (value := fields_map.get(field)) is not None
                and _is_finite_number(value)
            ]
            if not fields:
                continue
            pub_dates = snap["pub_dates"]
            ann_date = (
                max(pub_dates)
                if pub_dates
                else report_period + _dt.timedelta(days=ann_date_approx_days)
            )
            rows.append({
                "symbol": symbol,
                "report_period": report_period,
                "ann_date": ann_date,
                "ann_date_is_approx": not bool(pub_dates),
                "report_version": stat,
                "period_kind": "single_quarter",
                "raw_fields": [{"name": f, "value": v} for f, v in raw_fields.items()
                               if v is not None and _is_finite_number(v)],
                "fields": [{"name": f, "value": v} for f, v in fields],
                "source": self.name,
            })
        df = pd.DataFrame(rows)
        return canonicalize(df, "financial")

    def _profit_data(self, session, code, year, quarter):
        return session.query_profit_data(code=code, year=year, quarter=quarter)

    def _map_profit(self, rec):
        rev = _num(rec, "MBRevenue")
        np_ = _num(rec, "profit_net", "netProfit")
        return {
            "revenue": rev,
            "net_profit": np_,
            "eps": _num(rec, "epsTTM"),  # no plain quarterly EPS; epsTTM as approximation
            "gross_margin": _num(rec, "gpMargin"),
            "roe": _num(rec, "roeAvg"),
        }

    def _balance_data(self, session, code, year, quarter):
        return session.query_balance_data(code=code, year=year, quarter=quarter)

    def _map_balance(self, rec):
        equity = _num(rec, "equity", "totalEquity", "totalShareholderEquity")
        assets = _num(rec, "totalAssets", "total_assets", "totalAsset")
        liabilities = _num(rec, "totalLiability", "totalLiabilities", "total_liabilities")
        return {
            "equity": equity,
            "total_assets": assets,
            "total_liabilities": liabilities,
            "debt_ratio": _num(rec, "liabilityToAsset"),
        }

    def _cash_flow_data(self, session, code, year, quarter):
        return session.query_cash_flow_data(code=code, year=year, quarter=quarter)

    def _map_cash_flow(self, rec):
        value = _num(
            rec,
            "netCashFlowFromOperatingActivities",
            "cashFlowFromOperatingActivities",
            "netOperateCashFlow",
            "NCFO",
        )
        return {"oper_cash_flow": value}

    # ------------------------------------------------------------------
    # earnings_notice (forecast + express)
    # ------------------------------------------------------------------

    @_empty_on_unavailable("earnings_notice")
    def fetch_earnings_notice(self, symbol: str) -> pd.DataFrame:
        code = to_bs_code(symbol)
        start = "2003-01-01"
        end = time.strftime("%Y-%m-%d")
        rows = []
        pacer = _QueryPacer()
        with _session() as session:
            forecast = pacer.call(
                session.query_forecast_report,
                code,
                start_date=start,
                end_date=end,
            )
            for rec in _rows(forecast):
                rows.append(self._forecast_row(rec, symbol))
            express = pacer.call(
                session.query_performance_express_report,
                code,
                start_date=start,
                end_date=end,
            )
            for rec in _rows(express):
                rows.append(self._express_row(rec, symbol))
        if not rows:
            return empty_frame("earnings_notice")
        df = pd.DataFrame(rows)
        return canonicalize(df, "earnings_notice")

    # ------------------------------------------------------------------
    # price_val (historical shares aligned to unadjusted daily close)
    # ------------------------------------------------------------------

    @_empty_on_unavailable("price_val")
    def fetch_price_val(self, symbol: str) -> pd.DataFrame:
        code = to_bs_code(symbol)
        today = _dt.date.today()
        share_points = []
        pacer = _QueryPacer()
        with _session() as session:
            for year in range(2007, today.year + 1):
                max_quarter = (today.month - 1) // 3 + 1 if year == today.year else 4
                for quarter in range(1, max_quarter + 1):
                    result = pacer.call(
                        session.query_profit_data,
                        code=code,
                        year=year,
                        quarter=quarter,
                    )
                    for rec in _rows(result):
                        report_period = _parse_date(rec.get("statDate"))
                        total = _num(rec, "totalShare", "totalShares")
                        floating = _num(rec, "liqaShare", "floatShare", "floatShares")
                        if report_period is not None and total is not None and floating is not None:
                            share_points.append((report_period, total, floating))
            rs = pacer.call(
                session.query_history_k_data_plus,
                code,
                "date,close",
                start_date="2007-01-01",
                end_date=today.isoformat(),
                frequency="d",
                adjustflag="3",
            )
            daily = list(_rows(rs))
        if not share_points or not daily:
            return empty_frame("price_val")
        share_points.sort(key=lambda item: item[0])
        point_dates = [item[0] for item in share_points]
        rows = []
        for rec in daily:
            trade_date = _parse_date(rec.get("date"))
            close = _num(rec, "close")
            if trade_date is None or close is None:
                continue
            index = bisect_right(point_dates, trade_date) - 1
            if index < 0:
                continue
            _, total_shares, float_shares = share_points[index]
            rows.append({
                "symbol": symbol,
                "trade_date": trade_date,
                "close": close,
                "total_shares": total_shares,
                "float_shares": float_shares,
                "source": self.name,
            })
        return canonicalize(pd.DataFrame(rows), "price_val")

    def _forecast_row(self, rec, symbol):
        yoy = _mid(
            _num(rec, "profitForcastChgPctUp"),
            _num(rec, "profitForcastChgPctDwn"),
        )
        return {
            "symbol": symbol,
            "ann_date": _parse_date(rec.get("profitForcastExpPubDate")),
            "report_period": _parse_date(rec.get("profitForcastExpStatDate")),
            "kind": "forecast",
            # baostock forecast gives YoY + range only, no absolute profit.
            "net_profit": None,
            "net_profit_yoy": yoy,
            "source": self.name,
        }

    def _express_row(self, rec, symbol):
        return {
            "symbol": symbol,
            "ann_date": _parse_date(rec.get("performanceExpPubDate")),
            "report_period": _parse_date(rec.get("performanceExpStatDate")),
            "kind": "express",
            # performance express does not expose absolute net profit in the current SDK schema.
            "net_profit": None,
            "net_profit_yoy": None,
            "source": self.name,
        }


def _mid(lo, hi):
    if lo is None and hi is None:
        return None
    lo = lo if lo is not None else hi
    hi = hi if hi is not None else lo
    return (lo + hi) / 2.0


def _parse_date(value):
    if value in (None, ""):
        return None
    try:
        return to_date(value)
    except (TypeError, ValueError):
        return None


def _is_finite_number(value):
    return value is not None and pd.notna(value) and float(value) not in (
        float("inf"),
        float("-inf"),
    )
