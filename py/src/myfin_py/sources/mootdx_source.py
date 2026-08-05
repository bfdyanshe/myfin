"""mootdx adapter: TDX (通达信) TCP primary source for A-share daily bars.

Plan uses the active fork `mootdx-plus` (0.12+, fixes the BSE 920 market
mapping); it installs under the same `mootdx` package name. No auth, low
ban risk, fastest quote path. Server list rotates - callers must fall back
down the priority chain (tencent -> tushare) on connection failure.

NOTE on units: TDX returns volume in 手 (100 shares) and amount in 元 for
A-share stocks, which matches the canonical schema directly.
"""

from __future__ import annotations

import datetime as _dt

import pandas as pd

from myfin_py.schema import canonicalize, empty_frame, normalize_symbol, to_date

try:
    from mootdx.quotes import Quotes
except Exception as exc:  # pragma: no cover - machine without SDK installed
    Quotes = None
    IMPORT_ERROR = f"mootdx SDK import failed (pip install mootdx / mootdx-plus): {exc}"
else:
    IMPORT_ERROR = None

from myfin_py.sources import BaseAdapter, SourceError  # noqa: E402


def _client():
    """Return a connected TDX std-market client (server list managed upstream)."""
    if Quotes is None:
        raise SourceError(IMPORT_ERROR, source="mootdx")
    try:
        client = Quotes.factory(market="std")
    except Exception as exc:  # noqa: BLE001 - server pool can be empty
        raise SourceError(f"mootdx client init failed: {exc}", source="mootdx")
    if client is None:
        raise SourceError("mootdx returned no client (server list unreachable)", source="mootdx")
    return client


def _bars(client, code6: str, start: str, end: str) -> pd.DataFrame:
    """Fetch daily bars for a date range.

    TDX API is offset-based (bars back from now). Request a generous window
    and filter by date locally. Signature differences between mootdx and
    mootdx-plus are handled defensively.
    """
    days = (_dt.date.today() - _dt.date.fromisoformat(start)).days
    count = min(days + 20, 800)  # TDX daily bars cap around 800 per request
    kwargs = dict(symbol=code6, frequency=9, offset=count)
    try:
        df = client.bars(**kwargs)
    except TypeError:
        kwargs.pop("offset")
        df = client.bars(**{**kwargs, "start": count})
    except Exception as exc:  # noqa: BLE001 - network/server errors
        raise SourceError(f"mootdx bars failed for {code6}: {exc}", source="mootdx")
    if df is None or df.empty:
        return empty_frame("daily")
    df = df.copy()
    df["trade_date"] = df["datetime"].map(to_date)
    for col in ("open", "high", "low", "close"):
        df[col] = pd.to_numeric(df[col], errors="coerce")
    # TDX returns 手 for vol; amount in 元 (canonical units already).
    df["volume"] = pd.to_numeric(df.get("vol"), errors="coerce")
    df["amount"] = pd.to_numeric(df.get("amount"), errors="coerce")
    df = df.dropna(subset=["close"])
    df = df[(df["trade_date"] >= _dt.date.fromisoformat(start)) & (df["trade_date"] <= _dt.date.fromisoformat(end))]
    return df


class MootdxSource(BaseAdapter):
    name = "mootdx"
    package = "myfin_py.sources.mootdx_source"
    IMPORT_ERROR = IMPORT_ERROR
    # config/sources.yaml declares price_val too; TDX quote parsing is
    # pending (M3) so it stays out of capabilities until implemented.
    DATASETS = ["daily"]
    PROBE_SYMBOL = "600519"
    PROBE_LOOKBACK_DAYS = 5

    def fetch_daily(self, symbol: str, start: str, end: str) -> pd.DataFrame:
        code6 = normalize_symbol(symbol).split(".")[0]
        client = _client()
        try:
            df = _bars(client, code6, start, end)
        finally:
            try:
                client.close()
            except Exception:  # pragma: no cover - close is best-effort
                pass
        if df is None or df.empty:
            return empty_frame("daily")
        df["symbol"] = normalize_symbol(symbol)
        df["source"] = self.name
        df = df[["trade_date", "open", "high", "low", "close", "volume", "amount", "symbol", "source"]]
        df = df.drop_duplicates(subset=["trade_date"]).sort_values("trade_date")
        return canonicalize(df, "daily")
