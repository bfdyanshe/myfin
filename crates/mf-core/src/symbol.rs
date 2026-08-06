//! 证券代码与交易所、市场分类。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Exchange {
    /// 上交所
    Sse,
    /// 深交所
    Szse,
    /// 北交所
    Bse,
}

impl Exchange {
    pub fn as_str(&self) -> &'static str {
        match self {
            Exchange::Sse => "SH",
            Exchange::Szse => "SZ",
            Exchange::Bse => "BJ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Market {
    /// 沪市主板
    MainSse,
    /// 深市主板
    MainSzse,
    /// 科创板
    Star,
    /// 创业板
    ChiNext,
    /// 北交所
    Bse,
}

/// 股票代码（A 股）。统一格式：6 位数字代码 + 交易所后缀，如 `600519.SH`、`000001.SZ`、`430047.BJ`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol {
    pub code: String,
    pub exchange: Exchange,
}

impl Symbol {
    pub fn new(code: impl Into<String>, exchange: Exchange) -> Self {
        Self {
            code: code.into(),
            exchange,
        }
    }

    /// 根据 6 位代码推断交易所/市场（沪深京通用规则）。
    ///
    /// - 60/68/9 开头 -> 上交所；688 为科创板
    /// - 00/002/003/30 -> 深交所；300 为创业板
    /// - 43/83/87/92 开头 -> 北交所
    pub fn from_code(code: &str) -> Option<Self> {
        if code.len() != 6 {
            return None;
        }
        let exchange = match code.as_bytes()[0] {
            b'6' | b'9' => Exchange::Sse,
            b'0' | b'3' => Exchange::Szse,
            b'4' | b'8' => Exchange::Bse,
            _ => return None,
        };
        Some(Symbol::new(code, exchange))
    }

    pub fn market(&self) -> Market {
        match self.exchange {
            Exchange::Sse => {
                if self.code.starts_with("688") {
                    Market::Star
                } else {
                    Market::MainSse
                }
            }
            Exchange::Szse => {
                if self.code.starts_with("300") || self.code.starts_with("301") {
                    Market::ChiNext
                } else {
                    Market::MainSzse
                }
            }
            Exchange::Bse => Market::Bse,
        }
    }

    /// `600519.SH` 形式
    pub fn ts_code(&self) -> String {
        format!("{}.{}", self.code, self.exchange.as_str())
    }
}

impl std::fmt::Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.ts_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_exchange() {
        assert_eq!(Symbol::from_code("600519").unwrap().exchange, Exchange::Sse);
        assert_eq!(
            Symbol::from_code("000001").unwrap().exchange,
            Exchange::Szse
        );
        assert_eq!(
            Symbol::from_code("300750").unwrap().exchange,
            Exchange::Szse
        );
        assert_eq!(Symbol::from_code("430047").unwrap().exchange, Exchange::Bse);
        assert!(Symbol::from_code("123456").is_none());
    }

    #[test]
    fn infer_market() {
        assert_eq!(Symbol::from_code("688981").unwrap().market(), Market::Star);
        assert_eq!(
            Symbol::from_code("600519").unwrap().market(),
            Market::MainSse
        );
        assert_eq!(
            Symbol::from_code("300750").unwrap().market(),
            Market::ChiNext
        );
        assert_eq!(Symbol::from_code("430047").unwrap().market(), Market::Bse);
    }
}
