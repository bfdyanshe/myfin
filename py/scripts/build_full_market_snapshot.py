#!/usr/bin/env python3
"""构建指定截面的全市场筛选输入。

股票池使用 Baostock 基础信息和申万历史行业分类；财务使用新浪公开财务
报告接口，日线使用 AKShare 的不复权日线接口，业绩预告使用 AKShare 的
批量接口。所有网络结果都按标的缓存，单个接口失败只影响对应数据。

该脚本是一次性批量构建器，不改变默认同步链。它把结果写入 canonical
Parquet 目录，随后可直接由 ``mfctl screen --all`` 读取。
"""

from __future__ import annotations

import argparse
import datetime as dt
import io
import json
import re
import signal
import sys
import time
import warnings
from bisect import bisect_right
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any

import pandas as pd
import pyarrow.parquet as pq
import requests

from myfin_py.schema import canonicalize, normalize_symbol, to_arrow_table, to_date

try:
    import akshare as ak
except Exception as exc:  # pragma: no cover - execution environment dependent
    ak = None
    AKSHARE_ERROR = str(exc)
else:
    AKSHARE_ERROR = None

try:
    import baostock as bs
except Exception as exc:  # pragma: no cover - execution environment dependent
    bs = None
    BAOSTOCK_ERROR = str(exc)
else:
    BAOSTOCK_ERROR = None


AS_OF_DEFAULT = dt.date.today()
FINANCIAL_FIELDS = {
    "BIZTOTINCO": "revenue",
    "PARENETP": "net_profit",
    "RIGHAGGR": "equity",
    "TOTASSET": "total_assets",
    "TOTLIAB": "total_liabilities",
    "MANANETR": "oper_cash_flow",
    "EPSBASIC": "eps",
    "NAPS": "bps",
    "SGPMARGIN": "gross_margin",
    "ROEWEIGHTED": "roe",
    "ROEAVG": "roe",
    "ASSLIABRT": "debt_ratio",
}
ADDITIVE_FIELDS = {"revenue", "net_profit", "oper_cash_flow", "eps"}
PERCENT_FIELDS = {"gross_margin", "roe", "debt_ratio"}


def quiet_call(function, *args, **kwargs):
    """抑制 AKShare 的进度输出，保留异常给调用方。"""
    with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
        return function(*args, **kwargs)


def parse_float(value: Any) -> float | None:
    if value is None or pd.isna(value):
        return None
    text = str(value).strip().replace(",", "").replace("%", "")
    if text in {"", "-", "--", "None", "nan", "NaN", "无"}:
        return None
    match = re.search(r"[-+]?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?", text)
    if not match:
        return None
    try:
        number = float(match.group())
    except ValueError:
        return None
    return number if pd.notna(number) and abs(number) != float("inf") else None


def parse_date(value: Any) -> dt.date | None:
    if value is None or (isinstance(value, float) and pd.isna(value)):
        return None
    text = str(value).strip()
    if not text or text in {"-", "--", "None", "nan", "NaT"}:
        return None
    try:
        return to_date(text.replace("/", "-"))
    except (TypeError, ValueError):
        return None


def code6(value: Any) -> str | None:
    text = str(value).strip().lower()
    match = re.search(r"(?<!\d)(\d{6})(?!\d)", text)
    if not match:
        return None
    return match.group(1)


def symbol_from_code(value: Any) -> str | None:
    six = code6(value)
    if six is None:
        return None
    try:
        return normalize_symbol(six)
    except ValueError:
        return None


def stock_code(symbol: str) -> str:
    return symbol.split(".", 1)[0]


def exchange_code(symbol: str) -> str:
    return symbol.split(".", 1)[1].lower()


def quarter_ends(as_of: dt.date, count: int = 8) -> list[dt.date]:
    year = as_of.year
    month = ((as_of.month - 1) // 3) * 3 + 3
    result = []
    for _ in range(count):
        day = [31, 30, 30, 31][(month // 3) - 1]
        result.append(dt.date(year, month, day))
        month -= 3
        if month == 0:
            month = 12
            year -= 1
    return result


def load_local_env(root: Path) -> None:
    """仅在进程没有设置时读取 .env，绝不打印凭据。"""
    env_file = root / ".env"
    if not env_file.is_file():
        return
    for line in env_file.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip('"').strip("'")
        if key in {"BAOSTOCK_USER", "BAOSTOCK_PASSWORD"}:
            import os

            os.environ.setdefault(key, value)


def fetch_quotes() -> tuple[pd.DataFrame, list[str]]:
    if ak is None:
        raise RuntimeError(f"AKShare 不可用：{AKSHARE_ERROR}")
    frame = quiet_call(ak.stock_zh_a_spot_tx)
    if frame is None or frame.empty:
        raise RuntimeError("腾讯实时行情返回空表")
    frame = frame.copy()
    frame["_code"] = frame.get("code", frame.get("代码", "")).map(code6)
    frame = frame[frame["_code"].notna()]
    frame = frame[~frame["_code"].str.startswith(("4", "8", "9"))]
    frame["_cap_yuan"] = pd.to_numeric(frame.get("zsz"), errors="coerce") * 1.0e8
    frame["_float_cap_yuan"] = pd.to_numeric(frame.get("ltsz"), errors="coerce") * 1.0e8
    frame["_close"] = pd.to_numeric(frame.get("zxj"), errors="coerce")
    frame = frame[(frame["_cap_yuan"] >= 5.0e9) & (frame["_close"] > 0)]
    frame = frame.drop_duplicates("_code").reset_index(drop=True)
    return frame, [symbol_from_code(value) for value in frame["_code"]]


def fetch_basic(as_of: dt.date, symbols: set[str]) -> tuple[dict[str, dict[str, Any]], str]:
    """读取 Baostock 股票基础信息；登录/查询失败时返回空映射。"""
    if bs is None:
        return {}, f"Baostock SDK 不可用：{BAOSTOCK_ERROR}"
    import os

    user = os.environ.get("BAOSTOCK_USER", "anonymous")
    password = os.environ.get("BAOSTOCK_PASSWORD", "123456")
    login = None
    try:
        login = bs.login(user_id=user, password=password)
        if login.error_code != "0" and (user != "anonymous" or password != "123456"):
            bs.logout()
            login = bs.login(user_id="anonymous", password="123456")
        if login.error_code != "0":
            return {}, f"Baostock 登录失败 {login.error_code}: {login.error_msg}"
        result = bs.query_stock_basic()
        if result.error_code != "0":
            return {}, f"Baostock stock_basic 失败 {result.error_code}: {result.error_msg}"
        rows = []
        while result.next():
            rows.append(dict(zip(result.fields, result.get_row_data())))
        output = {}
        for row in rows:
            symbol = symbol_from_code(row.get("code"))
            if symbol not in symbols:
                continue
            listed = parse_date(row.get("ipoDate"))
            if listed is None or listed > as_of:
                continue
            output[symbol] = {
                "name": row.get("code_name") or None,
                "listed_date": listed,
                "delisted_date": parse_date(row.get("outDate")),
                "status": row.get("status"),
            }
        return output, "ok"
    except Exception as exc:  # noqa: BLE001 - 单个股票池源失败不阻断批量结果
        return {}, f"Baostock stock_basic 异常：{exc}"
    finally:
        if login is not None:
            try:
                bs.logout()
            except Exception:
                pass


def open_baostock() -> tuple[Any | None, str]:
    if bs is None:
        return None, f"Baostock SDK 不可用：{BAOSTOCK_ERROR}"
    import os

    user = os.environ.get("BAOSTOCK_USER", "anonymous")
    password = os.environ.get("BAOSTOCK_PASSWORD", "123456")
    try:
        login = bs.login(user_id=user, password=password)
        if login.error_code != "0" and (user != "anonymous" or password != "123456"):
            try:
                bs.logout()
            except Exception:
                pass
            login = bs.login(user_id="anonymous", password="123456")
        if login.error_code != "0":
            return None, f"Baostock 日线登录失败 {login.error_code}: {login.error_msg}"
        return bs, "ok"
    except Exception as exc:  # noqa: BLE001
        return None, f"Baostock 日线登录异常：{exc}"


def reconnect_baostock() -> None:
    """关闭可能卡住的 Baostock socket，并重新建立匿名/配置会话。"""
    if bs is None:
        return
    import os

    try:
        bs.logout()
    except Exception:
        pass
    user = os.environ.get("BAOSTOCK_USER", "anonymous")
    password = os.environ.get("BAOSTOCK_PASSWORD", "123456")
    login = bs.login(user_id=user, password=password)
    if login.error_code != "0" and (user != "anonymous" or password != "123456"):
        try:
            bs.logout()
        except Exception:
            pass
        bs.login(user_id="anonymous", password="123456")


class _BaostockTimeout(Exception):
    pass


def _alarm_handler(signum, frame):  # noqa: ARG001
    raise _BaostockTimeout("Baostock 单次日线请求超过 25 秒")


def fetch_industries(as_of: dt.date, symbols: set[str]) -> tuple[dict[str, str], str]:
    url = "https://www.swsresearch.com/swindex/pdf/SwClass2021/StockClassifyUse_stock.xls"
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            response = requests.get(url, timeout=30, verify=False)
        response.raise_for_status()
        frame = pd.read_excel(io.BytesIO(response.content), dtype=str)
        frame = frame.rename(columns={
            "股票代码": "code",
            "计入日期": "start_date",
            "行业代码": "industry",
        })
        frame["symbol"] = frame["code"].map(symbol_from_code)
        frame["start_date"] = frame["start_date"].map(parse_date)
        frame = frame[frame["symbol"].isin(symbols)]
        frame = frame[frame["start_date"].notna() & (frame["start_date"] <= as_of)]
        frame = frame.sort_values(["symbol", "start_date"])
        frame = frame.drop_duplicates("symbol", keep="last")
        return {
            row.symbol: str(row.industry)
            for row in frame.itertuples()
            if row.industry and str(row.industry) != "nan"
        }, "ok"
    except Exception as exc:  # noqa: BLE001 - 行业缺失可由 UNKNOWN 分组继续
        return {}, f"申万行业分类失败：{exc}"


def sina_report_rows(session: requests.Session, code: str, as_of: dt.date, cache: Path) -> tuple[list[dict[str, Any]], str | None]:
    if cache.is_file():
        try:
            return json.loads(cache.read_text(encoding="utf-8")), None
        except (OSError, json.JSONDecodeError):
            pass
    exchange = "sh" if code.startswith(("5", "6", "9")) else "sz"
    url = "https://quotes.sina.cn/cn/api/openapi.php/CompanyFinanceService.getFinanceReport2022"
    params = {"paperCode": f"{exchange}{code}", "source": "gjzb", "type": "0", "page": "1", "num": "1000"}
    try:
        response = session.get(url, params=params, timeout=30)
        response.raise_for_status()
        payload = response.json()
        report_list = payload.get("result", {}).get("data", {}).get("report_list", {})
        rows = []
        for report_key, report in report_list.items():
            report_period = parse_date(report_key)
            ann_date = parse_date(report.get("publish_date"))
            if report_period is None or ann_date is None or ann_date > as_of:
                continue
            values: dict[str, float] = {}
            for item in report.get("data", []):
                field = FINANCIAL_FIELDS.get(str(item.get("item_field", "")).upper())
                value = parse_float(item.get("item_value"))
                if field is None or value is None:
                    continue
                if field in PERCENT_FIELDS and abs(value) > 1.0:
                    value /= 100.0
                if field not in values or str(item.get("item_field", "")).upper() == "ROEWEIGHTED":
                    values[field] = value
            if values:
                rows.append({
                    "report_period": report_period.isoformat(),
                    "ann_date": ann_date.isoformat(),
                    "report_version": str(report_key),
                    "raw": values,
                })
        rows.sort(key=lambda row: row["report_period"])
        transformed = []
        previous: dict[int, dict[str, float]] = {}
        for row in rows:
            report_period = dt.date.fromisoformat(row["report_period"])
            raw = row["raw"]
            fields = dict(raw)
            quarter = (report_period.month - 1) // 3 + 1
            if quarter > 1:
                prior = previous.get(report_period.year, {})
                for field in ADDITIVE_FIELDS:
                    if field in fields and field in prior:
                        fields[field] -= prior[field]
            previous[report_period.year] = dict(raw)
            transformed.append({**row, "fields": fields})
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text(json.dumps(transformed, ensure_ascii=False), encoding="utf-8")
        return transformed, None
    except Exception as exc:  # noqa: BLE001 - 单标的失败留空并继续
        return [], f"新浪财务 {code}: {exc}"


def daily_rows(code: str, start: dt.date, as_of: dt.date, cache: Path, session: Any | None) -> tuple[list[dict[str, Any]], str | None]:
    if cache.is_file():
        try:
            cached = json.loads(cache.read_text(encoding="utf-8"))
            if isinstance(cached, dict):
                return cached.get("rows", []), cached.get("error")
            return cached, None
        except (OSError, json.JSONDecodeError):
            pass
    try:
        if session is not None:
            # Baostock 注册表约定的最小查询间隔，避免批量补数触发服务端限流。
            time.sleep(0.8)
        rows = []
        if session is not None:
            old_handler = signal.signal(signal.SIGALRM, _alarm_handler)
            signal.setitimer(signal.ITIMER_REAL, 25.0)
            try:
                result = session.query_history_k_data_plus(
                    f"{exchange_code(normalize_symbol(code))}.{code}",
                    "date,open,high,low,close,volume,amount",
                    start_date=start.isoformat(),
                    end_date=as_of.isoformat(),
                    frequency="d",
                    adjustflag="3",
                )
            finally:
                signal.setitimer(signal.ITIMER_REAL, 0)
                signal.signal(signal.SIGALRM, old_handler)
            if result.error_code != "0":
                error = f"Baostock 日线 {code} {result.error_code}: {result.error_msg}"
                cache.parent.mkdir(parents=True, exist_ok=True)
                cache.write_text(json.dumps({"rows": [], "error": error}, ensure_ascii=False), encoding="utf-8")
                return [], error
            while result.next():
                record = dict(zip(result.fields, result.get_row_data()))
                trade_date = parse_date(record.get("date"))
                close = parse_float(record.get("close"))
                if trade_date is None or close is None or close <= 0:
                    continue
                rows.append({
                    "trade_date": trade_date.isoformat(),
                    "open": parse_float(record.get("open")),
                    "high": parse_float(record.get("high")),
                    "low": parse_float(record.get("low")),
                    "close": close,
                    "volume": (parse_float(record.get("volume")) or 0.0) / 100.0,
                    "amount": parse_float(record.get("amount")) or 0.0,
                })
        elif ak is not None:
            frame = quiet_call(
                ak.stock_zh_a_hist,
                symbol=code,
                period="daily",
                start_date=start.strftime("%Y%m%d"),
                end_date=as_of.strftime("%Y%m%d"),
                adjust="",
            )
            if frame is not None:
                for record in frame.to_dict(orient="records"):
                    trade_date = parse_date(record.get("日期"))
                    close = parse_float(record.get("收盘"))
                    if trade_date is None or trade_date > as_of or close is None or close <= 0:
                        continue
                    rows.append({
                        "trade_date": trade_date.isoformat(),
                        "open": parse_float(record.get("开盘")),
                        "high": parse_float(record.get("最高")),
                        "low": parse_float(record.get("最低")),
                        "close": close,
                        "volume": parse_float(record.get("成交量")) or 0.0,
                        "amount": parse_float(record.get("成交额")) or 0.0,
                    })
        else:
            return [], f"Baostock 与 AKShare 日线均不可用"
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text(json.dumps(rows, ensure_ascii=False), encoding="utf-8")
        return rows, None
    except _BaostockTimeout as exc:
        error = f"Baostock 日线 {code}: {exc}"
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text(json.dumps({"rows": [], "error": error}, ensure_ascii=False), encoding="utf-8")
        try:
            reconnect_baostock()
        except Exception as reconnect_error:  # noqa: BLE001
            error = f"{error}；重连失败：{reconnect_error}"
        return [], error
    except Exception as exc:  # noqa: BLE001 - 单标的失败留空并继续
        error = f"Baostock/AKShare 日线 {code}: {exc}"
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text(json.dumps({"rows": [], "error": error}, ensure_ascii=False), encoding="utf-8")
        return [], error


def financial_frame(symbol: str, records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    rows = []
    for record in records:
        raw = record.get("raw", {})
        fields = record.get("fields", {})
        rows.append({
            "symbol": symbol,
            "report_period": dt.date.fromisoformat(record["report_period"]),
            "ann_date": dt.date.fromisoformat(record["ann_date"]),
            "ann_date_is_approx": False,
            "report_version": record.get("report_version"),
            "period_kind": "single_quarter",
            "raw_fields": [{"name": key, "value": value} for key, value in raw.items()],
            "fields": [{"name": key, "value": value} for key, value in fields.items()],
            "source": "sina",
        })
    return rows


def fields_map(record: dict[str, Any]) -> dict[str, float]:
    fields = record.get("fields", {})
    if isinstance(fields, dict):
        return {str(key): float(value) for key, value in fields.items()}
    return {str(item["name"]): float(item["value"]) for item in fields}


def price_rows(symbol: str, daily: list[dict[str, Any]], financial: list[dict[str, Any]], quote: pd.Series) -> list[dict[str, Any]]:
    close = parse_float(quote.get("_close")) or 0.0
    total = parse_float(quote.get("_cap_yuan"))
    floating = parse_float(quote.get("_float_cap_yuan"))
    total = total / close if total and close > 0 else None
    floating = floating / close if floating and close > 0 else None
    share_points = []
    for record in financial:
        values = fields_map(record)
        equity, bps = values.get("equity"), values.get("bps")
        if equity and equity > 0 and bps and bps > 0:
            share_points.append((dt.date.fromisoformat(record["ann_date"]), equity / bps))
    share_points.sort()
    point_dates = [point[0] for point in share_points]
    rows = []
    for record in daily:
        trade_date = dt.date.fromisoformat(record["trade_date"])
        index = bisect_right(point_dates, trade_date) - 1
        historical_total = share_points[index][1] if index >= 0 else total
        if historical_total is None:
            continue
        rows.append({
            "symbol": symbol,
            "trade_date": trade_date,
            "close": record["close"],
            "total_shares": historical_total,
            "float_shares": floating or historical_total,
            "source": "sina",
        })
    if not rows and close > 0 and total:
        rows.append({
            "symbol": symbol,
            "trade_date": dt.date.today(),
            "close": close,
            "total_shares": total,
            "float_shares": floating or total,
            "source": "sina",
        })
    return rows


def write_rows(root: Path, dataset: str, rows: list[dict[str, Any]], date_column: str, prefix: str) -> int:
    if not rows:
        return 0
    frame = canonicalize(pd.DataFrame(rows), dataset)
    frame[date_column] = frame[date_column].map(parse_date)
    count = 0
    for year, group in frame.groupby(frame[date_column].map(lambda value: value.year), sort=True):
        target_dir = root / ("market" if dataset in {"daily", "price_val"} else "financial") / dataset / str(year)
        if dataset == "daily":
            target_dir = root / "market" / "daily" / str(year)
        elif dataset == "price_val":
            target_dir = root / "market" / "price_val" / str(year)
        elif dataset == "financial":
            target_dir = root / "financial" / str(year)
        elif dataset == "earnings_notice":
            target_dir = root / "financial" / str(year)
        target_dir.mkdir(parents=True, exist_ok=True)
        target = target_dir / f"{prefix}-{year}.parquet"
        pq.write_table(to_arrow_table(group.reset_index(drop=True), dataset), target, compression="zstd")
        count += len(group)
    return count


def fetch_earnings(as_of: dt.date, symbols: set[str], cache: Path) -> tuple[list[dict[str, Any]], list[str]]:
    if ak is None:
        return [], [f"AKShare 不可用：{AKSHARE_ERROR}"]
    all_rows = []
    errors = []
    for report_period in quarter_ends(as_of):
        path = cache / f"{report_period.isoformat()}.json"
        try:
            if path.is_file():
                records = json.loads(path.read_text(encoding="utf-8"))
            else:
                frame = quiet_call(ak.stock_yjyg_em, date=report_period.strftime("%Y%m%d"))
                records = frame.to_dict(orient="records") if frame is not None else []
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(records, ensure_ascii=False, default=str), encoding="utf-8")
            for record in records:
                indicator = str(record.get("预测指标", ""))
                if "归属于上市公司股东的净利润" not in indicator or "扣除" in indicator:
                    continue
                symbol = symbol_from_code(record.get("股票代码") or record.get("代码"))
                if symbol not in symbols:
                    continue
                ann_date = parse_date(record.get("公告日期") or record.get("发布日期")) or report_period
                if ann_date > as_of:
                    continue
                all_rows.append({
                    "symbol": symbol,
                    "ann_date": ann_date,
                    "report_period": parse_date(record.get("报告期") or record.get("报告日期")) or report_period,
                    "kind": "forecast",
                    "net_profit": parse_float(record.get("预测数值") or record.get("预告净利润最大值")),
                    "net_profit_yoy": parse_float(record.get("业绩变动幅度") or record.get("预告净利润变动幅度")),
                    "source": "akshare",
                })
        except Exception as exc:  # noqa: BLE001 - 缺一个季度不阻断其他季度
            errors.append(f"业绩预告 {report_period}: {exc}")
    dedup = {}
    for row in all_rows:
        key = (row["symbol"], row["report_period"])
        if key not in dedup or row["ann_date"] > dedup[key]["ann_date"]:
            dedup[key] = row
    return list(dedup.values()), errors


def snapshot_rows(as_of: dt.date, quotes: pd.DataFrame, basic: dict[str, dict[str, Any]], industries: dict[str, str]) -> list[dict[str, Any]]:
    rows = []
    for _, quote in quotes.iterrows():
        symbol = symbol_from_code(quote.get("_code"))
        if symbol is None:
            continue
        info = basic.get(symbol, {})
        listed = info.get("listed_date") or dt.date(2000, 1, 1)
        name = info.get("name") or quote.get("name")
        rows.append({
            "symbol": symbol,
            "effective_date": as_of,
            "name": name,
            "industry": industries.get(symbol, "UNKNOWN"),
            "is_st": bool(name and "ST" in str(name).upper()),
            "listed_date": listed,
            "delisted_date": info.get("delisted_date"),
            "is_suspended": False,
            "price_limit_up": None,
            "price_limit_down": None,
            "source": "baostock+tx+sw",
        })
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", default="data", type=Path)
    parser.add_argument("--as-of", type=dt.date.fromisoformat, default=AS_OF_DEFAULT)
    parser.add_argument("--limit", type=int, default=0, help="仅处理前 N 只，0 表示全市场")
    parser.add_argument("--batch-size", type=int, default=50)
    args = parser.parse_args()
    root = args.data_dir.resolve()
    as_of = args.as_of
    load_local_env(root.parent)
    build_dir = root / "universe" / as_of.isoformat()
    cache_dir = build_dir / "cache"
    build_dir.mkdir(parents=True, exist_ok=True)
    start = as_of - dt.timedelta(days=5 * 365 + 30)
    errors: list[str] = []

    quotes, symbols = fetch_quotes()
    symbols = [symbol for symbol in symbols if symbol]
    if args.limit > 0:
        symbols = symbols[:args.limit]
        quotes = quotes[quotes["_code"].map(symbol_from_code).isin(set(symbols))]
    symbol_set = set(symbols)
    basic, basic_status = fetch_basic(as_of, symbol_set)
    if basic_status != "ok":
        errors.append(basic_status)
    industries, industry_status = fetch_industries(as_of, symbol_set)
    if industry_status != "ok":
        errors.append(industry_status)

    snapshots = snapshot_rows(as_of, quotes, basic, industries)
    (build_dir / "snapshots.json").write_text(json.dumps(snapshots, ensure_ascii=False, default=str, indent=2), encoding="utf-8")
    earnings, earnings_errors = fetch_earnings(as_of, symbol_set, cache_dir / "earnings")
    errors.extend(earnings_errors)
    write_rows(root, "earnings_notice", earnings, "ann_date", f"full-market-{as_of:%Y%m%d}-earnings")

    session = requests.Session()
    daily_session, daily_status = open_baostock()
    if daily_status != "ok":
        errors.append(daily_status)
    financial_count = daily_count = price_count = 0
    financial_ok = daily_ok = 0
    financial_missing = []
    daily_missing = []
    for offset in range(0, len(symbols), args.batch_size):
        batch_symbols = symbols[offset : offset + args.batch_size]
        batch_financial, batch_daily, batch_prices = [], [], []
        for index, symbol in enumerate(batch_symbols, start=offset + 1):
            code = stock_code(symbol)
            financial, financial_error = sina_report_rows(
                session, code, as_of, cache_dir / "financial" / f"{code}.json"
            )
            financial_records = financial_frame(symbol, financial)
            daily, daily_error = daily_rows(
                code, start, as_of, cache_dir / "daily" / f"{code}.json", daily_session
            )
            if financial_error:
                financial_missing.append(symbol)
                errors.append(financial_error)
            else:
                financial_ok += 1
            if daily_error:
                daily_missing.append(symbol)
                errors.append(daily_error)
            else:
                daily_ok += 1
            batch_financial.extend(financial_records)
            daily_source = "baostock" if daily_session is not None else "akshare"
            for item in daily:
                batch_daily.append({"symbol": symbol, **item, "source": daily_source})
            quote = quotes.loc[quotes["_code"] == code].iloc[0]
            batch_prices.extend(price_rows(symbol, daily, financial, quote))
            if index % 25 == 0 or index == len(symbols):
                print(f"处理 {index}/{len(symbols)}: {symbol} 财务={len(financial_records)} 日线={len(daily)}", flush=True)
        financial_count += write_rows(root, "financial", batch_financial, "report_period", f"full-market-{as_of:%Y%m%d}-financial-{offset:05d}")
        daily_count += write_rows(root, "daily", batch_daily, "trade_date", f"full-market-{as_of:%Y%m%d}-daily-{offset:05d}")
        price_count += write_rows(root, "price_val", batch_prices, "trade_date", f"full-market-{as_of:%Y%m%d}-price-{offset:05d}")

    summary = {
        "as_of": as_of.isoformat(),
        "universe_count": len(snapshots),
        "market_cap_threshold_yuan": 5_000_000_000,
        "history_start": start.isoformat(),
        "financial_symbols_ok": financial_ok,
        "daily_symbols_ok": daily_ok,
        "financial_rows": financial_count,
        "daily_rows": daily_count,
        "price_val_rows": price_count,
        "earnings_notice_rows": len(earnings),
        "financial_missing_symbols": sorted(set(financial_missing)),
        "daily_missing_symbols": sorted(set(daily_missing)),
        "industry_mapped_symbols": len(industries),
        "baostock_status": basic_status,
        "warnings": sorted(set(errors))[:100],
        "warning_count": len(errors),
        "adj_factor": "批量构建未拉取；技术指标使用不复权价格，后续可按候选补齐 Baostock 后复权因子",
        "snapshots_path": str(build_dir / "snapshots.json"),
    }
    (build_dir / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2, default=str), encoding="utf-8")
    if daily_session is not None:
        try:
            bs.logout()
        except Exception:
            pass
    print(json.dumps(summary, ensure_ascii=False, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
