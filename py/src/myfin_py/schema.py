"""Canonical schema: column constants and Parquet column definitions.

Field names must match crates/mf-core (bar.rs / financial.rs / valuation.rs)
exactly. All adapters emit DataFrames with these stable column orders.
"""

import datetime as _dt

import pandas as pd
import pyarrow as pa

# ---------------------------------------------------------------------------
# Dataset column orders (stable output contract)
# ---------------------------------------------------------------------------

DAILY_COLUMNS = ["symbol", "trade_date", "open", "high", "low", "close", "volume", "amount", "source"]
ADJ_FACTOR_COLUMNS = ["symbol", "ex_date", "cum_factor", "source"]
FINANCIAL_COLUMNS = ["symbol", "report_period", "ann_date", "fields", "source"]
EARNINGS_COLUMNS = ["symbol", "ann_date", "report_period", "kind", "net_profit", "net_profit_yoy", "source"]
PRICE_VAL_COLUMNS = ["symbol", "trade_date", "close", "total_shares", "float_shares", "source"]

COLUMNS = {
    "daily": DAILY_COLUMNS,
    "adj_factor": ADJ_FACTOR_COLUMNS,
    "financial": FINANCIAL_COLUMNS,
    "earnings_notice": EARNINGS_COLUMNS,
    "price_val": PRICE_VAL_COLUMNS,
}

# Financial field enum (FinancialField in crates/mf-core/src/financial.rs).
CANONICAL_FIELDS = [
    "revenue",
    "net_profit",
    "equity",
    "total_assets",
    "total_liabilities",
    "oper_cash_flow",
    "eps",
    "bps",
    "gross_margin",
    "roe",
    "debt_ratio",
]

# ---------------------------------------------------------------------------
# Parquet schemas
# ---------------------------------------------------------------------------

_STRING = pa.string()
_DATE = pa.date32()
_DOUBLE = pa.float64()
_FIELD_STRUCT = pa.struct([pa.field("name", _STRING), pa.field("value", _DOUBLE)])

ARROW_SCHEMAS = {
    "daily": pa.schema(
        [
            pa.field("symbol", _STRING),
            pa.field("trade_date", _DATE),
            pa.field("open", _DOUBLE),
            pa.field("high", _DOUBLE),
            pa.field("low", _DOUBLE),
            pa.field("close", _DOUBLE),
            pa.field("volume", _DOUBLE),
            pa.field("amount", _DOUBLE),
            pa.field("source", _STRING),
        ]
    ),
    "adj_factor": pa.schema(
        [
            pa.field("symbol", _STRING),
            pa.field("ex_date", _DATE),
            pa.field("cum_factor", _DOUBLE),
            pa.field("source", _STRING),
        ]
    ),
    "financial": pa.schema(
        [
            pa.field("symbol", _STRING),
            pa.field("report_period", _DATE),
            pa.field("ann_date", _DATE),
            pa.field("fields", pa.list_(pa.field("item", _FIELD_STRUCT))),
            pa.field("source", _STRING),
        ]
    ),
    "earnings_notice": pa.schema(
        [
            pa.field("symbol", _STRING),
            pa.field("ann_date", _DATE),
            pa.field("report_period", _DATE),
            pa.field("kind", _STRING),
            pa.field("net_profit", _DOUBLE),
            pa.field("net_profit_yoy", _DOUBLE),
            pa.field("source", _STRING),
        ]
    ),
    "price_val": pa.schema(
        [
            pa.field("symbol", _STRING),
            pa.field("trade_date", _DATE),
            pa.field("close", _DOUBLE),
            pa.field("total_shares", _DOUBLE),
            pa.field("float_shares", _DOUBLE),
            pa.field("source", _STRING),
        ]
    ),
}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def empty_frame(dataset: str) -> pd.DataFrame:
    """Empty DataFrame with the canonical column order for `dataset`."""
    return pd.DataFrame(columns=COLUMNS[dataset])


def canonicalize(df: pd.DataFrame, dataset: str) -> pd.DataFrame:
    """Reorder/trim columns to the canonical order for `dataset`."""
    return df.reindex(columns=COLUMNS[dataset])


def to_arrow_table(df: pd.DataFrame, dataset: str) -> pa.Table:
    """Convert a canonical DataFrame to a pyarrow Table.

    Uses from_pylist with an explicit schema so date objects and the
    `financial.fields` list-of-struct column convert deterministically.
    """
    records = df.to_dict(orient="records")
    return pa.Table.from_pylist(records, schema=ARROW_SCHEMAS[dataset])


def to_date(value) -> _dt.date:
    """Coerce a str (YYYY-MM-DD) / date / datetime / Timestamp to date."""
    if isinstance(value, _dt.datetime):
        return value.date()
    if isinstance(value, _dt.date):
        return value
    return _dt.date.fromisoformat(str(value)[:10])


def normalize_symbol(code: str) -> str:
    """Normalize any A-share code variant to canonical `600519.SH` form.

    Accepts `600519.SH`, `600519`, `sh.600519`, `SH600519`, `600519.SH`.
    Raises ValueError for unparseable codes.
    """
    s = code.strip().lower().replace(" ", "")
    if s.endswith(".sh") or s.endswith(".sz") or s.endswith(".bj"):
        code6, ex = s[:-3], s[-2:]
        return f"{code6}.{ex.upper()}"
    if len(s) == 9 and s[2] == ".":
        return normalize_symbol(f"{s[3:]}.{s[:2]}")
    if len(s) == 8 and (s.startswith("sh") or s.startswith("sz") or s.startswith("bj")):
        return f"{s[2:]}.{s[:2].upper()}"
    if len(s) == 6 and s.isdigit():
        ex = "BJ" if s[0] in "48" else ("SH" if s[0] in "69" else "SZ")
        return f"{s}.{ex}"
    raise ValueError(f"cannot normalize symbol: {code!r}")
