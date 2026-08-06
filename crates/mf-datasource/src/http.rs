//! Rust 原生 HTTP 数据源适配器。
//!
//! 当前支持腾讯财经与 Tushare 的不复权日线。适配器只负责请求、
//! 限流和字段规范化；存储与优先级链由上层编排。

use std::future::Future;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use mf_core::{DailyBar, Error, PriceVal, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::dataset::{Dataset, DatasetProbe, DatasetSpec};
use crate::registry::{Auth, Probe, SourceConfig};
use crate::source::{HealthReport, Source, SourceCapabilities};

const TENCENT_KLINE_URL: &str = "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get";
const TUSHARE_API_URL: &str = "https://api.tushare.pro";

/// 已登记的 Rust HTTP 源。
pub enum HttpAdapter {
    Tencent(TencentSource),
    Tushare(TushareSource),
}

impl HttpAdapter {
    /// 根据注册表条目构造适配器。token 只从注册表声明的环境变量读取。
    pub fn from_config(config: &SourceConfig) -> Result<Self> {
        match config.name.as_str() {
            "tencent" => Ok(Self::Tencent(TencentSource::from_config(config)?)),
            "tushare" => Ok(Self::Tushare(TushareSource::from_config(config)?)),
            name => Err(Error::source_err(name, "尚未注册 Rust HTTP 适配器")),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Tencent(_) => "tencent",
            Self::Tushare(_) => "tushare",
        }
    }

    pub async fn health_check(&self) -> HealthReport {
        match self {
            Self::Tencent(source) => source.health_check().await,
            Self::Tushare(source) => source.health_check().await,
        }
    }

    pub async fn fetch_daily(
        &self,
        symbol: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<DailyBar>> {
        match self {
            Self::Tencent(source) => source.fetch_daily(symbol, start, end).await,
            Self::Tushare(source) => source.fetch_daily(symbol, start, end).await,
        }
    }
}

pub struct TencentSource {
    client: reqwest::Client,
    capabilities: SourceCapabilities,
    limiter: RateLimiter,
    probe: Probe,
}

impl TencentSource {
    fn from_config(config: &SourceConfig) -> Result<Self> {
        let probe = config
            .probe
            .clone()
            .ok_or_else(|| Error::source_err("tencent", "注册表缺少 probe"))?;
        Ok(Self {
            client: build_client("myfin/tencent")?,
            capabilities: capabilities(config),
            limiter: RateLimiter::new(config.rate_limit.min_interval_ms),
            probe,
        })
    }

    async fn fetch_daily_inner(
        &self,
        symbol: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<DailyBar>> {
        validate_symbol("tencent", symbol)?;
        if start > end {
            return Err(Error::source_err("tencent", "开始日期晚于结束日期"));
        }

        let mut upper = end;
        let mut rows = Vec::new();
        loop {
            let page = self.fetch_page(symbol, start, upper).await?;
            if page.is_empty() {
                break;
            }
            let oldest = page
                .iter()
                .map(|row| row.trade_date)
                .min()
                .ok_or_else(|| Error::source_err("tencent", "日线响应为空"))?;
            rows.extend(page);
            if oldest <= start {
                break;
            }
            let Some(next_end) = oldest.pred_opt() else {
                break;
            };
            if next_end >= upper {
                break;
            }
            upper = next_end;
        }

        rows.retain(|row| row.trade_date >= start && row.trade_date <= end);
        rows.sort_by_key(|row| row.trade_date);
        rows.dedup_by_key(|row| row.trade_date);
        Ok(rows)
    }

    async fn fetch_page(
        &self,
        symbol: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<DailyBar>> {
        self.limiter.wait().await;
        let provider_symbol = tencent_symbol(symbol)?;
        let param = format!("{provider_symbol},day,{start},{end},640,");
        let response = self
            .client
            .get(TENCENT_KLINE_URL)
            .query(&[("_var", "kline_day"), ("param", param.as_str())])
            .send()
            .await
            .map_err(|error| Error::source_err("tencent", error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| Error::source_err("tencent", error.to_string()))?;
        if !status.is_success() {
            return Err(Error::source_err(
                "tencent",
                format!("HTTP {}: {}", status, truncate(&body)),
            ));
        }
        parse_tencent_daily(&body, &provider_symbol)
    }
}

#[async_trait]
impl Source for TencentSource {
    fn capabilities(&self) -> &SourceCapabilities {
        &self.capabilities
    }

    async fn health_check(&self) -> HealthReport {
        probe_daily("tencent", &self.probe, async {
            let (start, end) = daily_window(&self.probe);
            self.fetch_daily_inner(&self.probe.symbol, start, end).await
        })
        .await
    }

    async fn fetch_daily(
        &self,
        symbol: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<DailyBar>> {
        self.fetch_daily_inner(symbol, start, end).await
    }

    async fn fetch_adj_factor(&self, _symbol: &str) -> Result<Vec<mf_core::AdjFactor>> {
        unsupported("tencent", Dataset::AdjFactor)
    }

    async fn fetch_financial(&self, _symbol: &str) -> Result<Vec<mf_core::FinancialData>> {
        unsupported("tencent", Dataset::Financial)
    }

    async fn fetch_earnings_notice(&self, _symbol: &str) -> Result<Vec<mf_core::EarningsNotice>> {
        unsupported("tencent", Dataset::EarningsNotice)
    }

    async fn fetch_price_val(
        &self,
        _symbol: &str,
        _start: NaiveDate,
        _end: NaiveDate,
    ) -> Result<Vec<PriceVal>> {
        unsupported("tencent", Dataset::PriceVal)
    }
}

pub struct TushareSource {
    client: reqwest::Client,
    capabilities: SourceCapabilities,
    limiter: RateLimiter,
    probe: Probe,
    token: String,
}

impl TushareSource {
    fn from_config(config: &SourceConfig) -> Result<Self> {
        let probe = config
            .probe
            .clone()
            .ok_or_else(|| Error::source_err("tushare", "注册表缺少 probe"))?;
        let env_var = match &config.auth {
            Auth::Token { env_var } => env_var,
            Auth::None => return Err(Error::source_err("tushare", "注册表应声明 token 鉴权")),
        };
        let token = std::env::var(env_var)
            .map_err(|_| Error::source_err("tushare", format!("缺少鉴权环境变量 {env_var}")))?;
        if token.trim().is_empty() {
            return Err(Error::source_err(
                "tushare",
                format!("鉴权环境变量 {env_var} 为空"),
            ));
        }
        Ok(Self {
            client: build_client("myfin/tushare")?,
            capabilities: capabilities(config),
            limiter: RateLimiter::new(config.rate_limit.min_interval_ms),
            probe,
            token,
        })
    }

    async fn fetch_daily_inner(
        &self,
        symbol: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<DailyBar>> {
        validate_symbol("tushare", symbol)?;
        if start > end {
            return Err(Error::source_err("tushare", "开始日期晚于结束日期"));
        }
        self.limiter.wait().await;
        let payload = serde_json::json!({
            "api_name": "daily",
            "token": self.token,
            "params": {
                "ts_code": symbol,
                "start_date": start.format("%Y%m%d").to_string(),
                "end_date": end.format("%Y%m%d").to_string(),
            },
            "fields": "ts_code,trade_date,open,high,low,close,vol,amount",
        });
        let response = self
            .client
            .post(TUSHARE_API_URL)
            .json(&payload)
            .send()
            .await
            .map_err(|error| Error::source_err("tushare", error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| Error::source_err("tushare", error.to_string()))?;
        if !status.is_success() {
            return Err(Error::source_err(
                "tushare",
                format!("HTTP {}: {}", status, truncate(&body)),
            ));
        }
        let response: TushareResponse = serde_json::from_str(&body)
            .map_err(|error| Error::source_err("tushare", format!("JSON 解析失败: {error}")))?;
        if response.code != 0 {
            return Err(Error::source_err(
                "tushare",
                response
                    .msg
                    .unwrap_or_else(|| format!("API code {}", response.code)),
            ));
        }
        let Some(data) = response.data else {
            return Ok(Vec::new());
        };
        parse_tushare_daily(data, symbol)
    }
}

#[async_trait]
impl Source for TushareSource {
    fn capabilities(&self) -> &SourceCapabilities {
        &self.capabilities
    }

    async fn health_check(&self) -> HealthReport {
        probe_daily("tushare", &self.probe, async {
            let (start, end) = daily_window(&self.probe);
            self.fetch_daily_inner(&self.probe.symbol, start, end).await
        })
        .await
    }

    async fn fetch_daily(
        &self,
        symbol: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<DailyBar>> {
        self.fetch_daily_inner(symbol, start, end).await
    }

    async fn fetch_adj_factor(&self, _symbol: &str) -> Result<Vec<mf_core::AdjFactor>> {
        unsupported("tushare", Dataset::AdjFactor)
    }

    async fn fetch_financial(&self, _symbol: &str) -> Result<Vec<mf_core::FinancialData>> {
        unsupported("tushare", Dataset::Financial)
    }

    async fn fetch_earnings_notice(&self, _symbol: &str) -> Result<Vec<mf_core::EarningsNotice>> {
        unsupported("tushare", Dataset::EarningsNotice)
    }

    async fn fetch_price_val(
        &self,
        _symbol: &str,
        _start: NaiveDate,
        _end: NaiveDate,
    ) -> Result<Vec<PriceVal>> {
        unsupported("tushare", Dataset::PriceVal)
    }
}

#[derive(Debug, Deserialize)]
struct TushareResponse {
    code: i64,
    #[serde(default)]
    msg: Option<String>,
    data: Option<TushareData>,
}

#[derive(Debug, Deserialize)]
struct TushareData {
    fields: Vec<String>,
    items: Vec<Vec<Value>>,
}

fn parse_tencent_daily(body: &str, symbol: &str) -> Result<Vec<DailyBar>> {
    let payload = body
        .split_once('=')
        .map(|(_, value)| value)
        .unwrap_or(body)
        .trim();
    let root: Value = serde_json::from_str(payload)
        .map_err(|error| Error::source_err("tencent", format!("JSON 解析失败: {error}")))?;
    let code = root.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        return Err(Error::source_err(
            "tencent",
            root.get("msg")
                .and_then(Value::as_str)
                .unwrap_or("接口返回错误"),
        ));
    }
    let Some(rows) = root
        .get("data")
        .and_then(|data| data.get(symbol))
        .and_then(|stock| stock.get("day"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    rows.iter()
        .map(|row| {
            let row = row
                .as_array()
                .ok_or_else(|| Error::source_err("tencent", "日线记录不是数组"))?;
            if row.len() < 6 {
                return Err(Error::source_err("tencent", "日线记录字段不足"));
            }
            Ok(DailyBar {
                symbol: canonical_symbol(symbol)?,
                trade_date: parse_date(&row[0], "tencent")?,
                open: parse_number(&row[1], "tencent", "open")?,
                close: parse_number(&row[2], "tencent", "close")?,
                high: parse_number(&row[3], "tencent", "high")?,
                low: parse_number(&row[4], "tencent", "low")?,
                volume: parse_number(&row[5], "tencent", "volume")?,
                amount: 0.0,
                source: "tencent".to_string(),
            })
        })
        .collect()
}

fn parse_tushare_daily(data: TushareData, requested_symbol: &str) -> Result<Vec<DailyBar>> {
    let ts_code = field_index(&data.fields, "ts_code", "tushare")?;
    let trade_date = field_index(&data.fields, "trade_date", "tushare")?;
    let open = field_index(&data.fields, "open", "tushare")?;
    let high = field_index(&data.fields, "high", "tushare")?;
    let low = field_index(&data.fields, "low", "tushare")?;
    let close = field_index(&data.fields, "close", "tushare")?;
    let volume = field_index(&data.fields, "vol", "tushare")?;
    let amount = field_index(&data.fields, "amount", "tushare")?;

    let mut rows = data
        .items
        .into_iter()
        .map(|row| {
            let source_symbol = value_to_string(row_value(&row, ts_code, "ts_code", "tushare")?)?;
            let symbol = if source_symbol.is_empty() {
                requested_symbol.to_string()
            } else {
                source_symbol
            };
            Ok(DailyBar {
                symbol: canonical_symbol(&symbol)?,
                trade_date: parse_date(
                    row_value(&row, trade_date, "trade_date", "tushare")?,
                    "tushare",
                )?,
                open: parse_number(row_value(&row, open, "open", "tushare")?, "tushare", "open")?,
                high: parse_number(row_value(&row, high, "high", "tushare")?, "tushare", "high")?,
                low: parse_number(row_value(&row, low, "low", "tushare")?, "tushare", "low")?,
                close: parse_number(
                    row_value(&row, close, "close", "tushare")?,
                    "tushare",
                    "close",
                )?,
                volume: parse_number(row_value(&row, volume, "vol", "tushare")?, "tushare", "vol")?,
                amount: parse_number(
                    row_value(&row, amount, "amount", "tushare")?,
                    "tushare",
                    "amount",
                )? * 1000.0,
                source: "tushare".to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    rows.sort_by_key(|row| row.trade_date);
    rows.dedup_by_key(|row| row.trade_date);
    Ok(rows)
}

fn field_index(fields: &[String], name: &str, source: &str) -> Result<usize> {
    fields
        .iter()
        .position(|field| field == name)
        .ok_or_else(|| Error::source_err(source, format!("响应缺少字段 {name}")))
}

fn row_value<'a>(row: &'a [Value], index: usize, field: &str, source: &str) -> Result<&'a Value> {
    row.get(index)
        .ok_or_else(|| Error::source_err(source, format!("字段 {field} 没有对应值")))
}

fn value_to_string(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        _ => Err(Error::Validation("字段不是字符串或数字".to_string())),
    }
}

fn parse_date(value: &Value, source: &str) -> Result<NaiveDate> {
    let date = value_to_string(value)?;
    NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(&date, "%Y%m%d"))
        .map_err(|error| Error::source_err(source, format!("日期 {date} 解析失败: {error}")))
}

fn parse_number(value: &Value, source: &str, field: &str) -> Result<f64> {
    let text = value_to_string(value)?;
    text.parse::<f64>().map_err(|error| {
        Error::source_err(
            source,
            format!("字段 {field} 的值 {text} 不是数字: {error}"),
        )
    })
}

fn validate_symbol(source: &str, symbol: &str) -> Result<()> {
    if symbol.is_empty()
        || !symbol
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.')
    {
        return Err(Error::source_err(source, "标的代码包含非法字符"));
    }
    Ok(())
}

fn canonical_symbol(symbol: &str) -> Result<String> {
    if let Some((code, exchange)) = symbol.split_once('.') {
        let exchange = exchange.to_ascii_uppercase();
        if matches!(exchange.as_str(), "SH" | "SZ" | "BJ")
            && code.len() == 6
            && code.chars().all(|ch| ch.is_ascii_digit())
        {
            return Ok(format!("{code}.{exchange}"));
        }
    }
    if symbol.len() == 8
        && symbol[..2].eq_ignore_ascii_case("sh")
        && symbol[2..].chars().all(|ch| ch.is_ascii_digit())
    {
        return Ok(format!("{}.SH", &symbol[2..]));
    }
    if symbol.len() == 8
        && symbol[..2].eq_ignore_ascii_case("sz")
        && symbol[2..].chars().all(|ch| ch.is_ascii_digit())
    {
        return Ok(format!("{}.SZ", &symbol[2..]));
    }
    Err(Error::source_err(
        "http",
        format!("无法规范化股票代码 {symbol}"),
    ))
}

fn tencent_symbol(symbol: &str) -> Result<String> {
    let canonical = canonical_symbol(symbol)?;
    let (code, exchange) = canonical
        .split_once('.')
        .ok_or_else(|| Error::source_err("tencent", "无法拆分规范化股票代码"))?;
    Ok(format!("{}{}", exchange.to_ascii_lowercase(), code))
}

fn build_client(user_agent: &str) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(user_agent)
        .build()
        .map_err(|error| Error::Other(format!("创建 HTTP 客户端失败: {error}")))
}

fn capabilities(config: &SourceConfig) -> SourceCapabilities {
    let specs = config
        .datasets
        .iter()
        .copied()
        .map(|dataset| DatasetSpec {
            dataset,
            incremental: dataset == Dataset::Daily,
            probe: if dataset == Dataset::Daily {
                config.probe.as_ref().map(|probe| DatasetProbe {
                    symbol: probe.symbol.clone(),
                    lookback_days: probe.lookback_days,
                })
            } else {
                None
            },
        })
        .collect();
    SourceCapabilities {
        name: config.name.clone(),
        datasets: config.datasets.clone(),
        specs,
    }
}

fn daily_window(probe: &Probe) -> (NaiveDate, NaiveDate) {
    let end = Utc::now().date_naive();
    let calendar_days = i64::from(probe.lookback_days.max(1)) * 2 + 5;
    (end - ChronoDuration::days(calendar_days), end)
}

async fn probe_daily<F>(source: &str, probe: &Probe, fetch: F) -> HealthReport
where
    F: Future<Output = Result<Vec<DailyBar>>>,
{
    let started = Instant::now();
    let (start, end) = daily_window(probe);
    match fetch.await {
        Ok(rows) if !rows.is_empty() => HealthReport {
            source: source.to_string(),
            ok: true,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(_) => HealthReport {
            source: source.to_string(),
            ok: false,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: Some(format!("{} 至 {} 返回空数据", start, end)),
        },
        Err(error) => HealthReport {
            source: source.to_string(),
            ok: false,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: Some(error.to_string()),
        },
    }
}

fn unsupported<T>(source: &str, dataset: Dataset) -> Result<T> {
    Err(Error::source_err(
        source,
        format!("当前适配器尚未实现数据集 {dataset}"),
    ))
}

fn truncate(value: &str) -> String {
    const LIMIT: usize = 300;
    if value.len() <= LIMIT {
        value.to_string()
    } else {
        format!("{}...", &value[..LIMIT])
    }
}

struct RateLimiter {
    interval: Duration,
    next_allowed: tokio::sync::Mutex<Instant>,
}

impl RateLimiter {
    fn new(min_interval_ms: u64) -> Self {
        Self {
            interval: Duration::from_millis(min_interval_ms),
            next_allowed: tokio::sync::Mutex::new(Instant::now()),
        }
    }

    async fn wait(&self) {
        let delay = {
            let mut next_allowed = self.next_allowed.lock().await;
            let now = Instant::now();
            let delay = next_allowed.saturating_duration_since(now);
            *next_allowed = now + delay + self.interval;
            delay
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tencent_unadjusted_daily() {
        let body = r#"kline_day={"code":0,"data":{"sh600519":{"day":[["2026-08-04","1328.360","1330.000","1340.000","1310.000","37450.000"]]}}}"#;
        let rows = parse_tencent_daily(body, "sh600519").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "600519.SH");
        assert_eq!(rows[0].volume, 37450.0);
        assert_eq!(rows[0].amount, 0.0);
    }

    #[test]
    fn converts_canonical_symbol_for_tencent() {
        assert_eq!(tencent_symbol("600519.SH").unwrap(), "sh600519");
        assert_eq!(tencent_symbol("sz000001").unwrap(), "sz000001");
    }

    #[test]
    fn parses_tushare_amount_in_thousand_yuan() {
        let body = r#"{"code":0,"msg":"","data":{"fields":["ts_code","trade_date","open","high","low","close","vol","amount"],"items":[["600519.SH","20260804",1328.36,1340.0,1310.0,1330.0,37450.0,123456.7]]}}"#;
        let response: TushareResponse = serde_json::from_str(body).unwrap();
        let rows = parse_tushare_daily(response.data.unwrap(), "600519.SH").unwrap();
        assert_eq!(rows[0].symbol, "600519.SH");
        assert_eq!(rows[0].amount, 123_456_700.0);
    }

    #[test]
    fn rejects_invalid_symbol() {
        assert!(validate_symbol("tencent", "sh600519,sh000001").is_err());
    }
}
