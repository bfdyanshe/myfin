//! 选股与回测共用的确定性指标。

/// 计算末端窗口的简单移动平均。
pub fn simple_moving_average(values: &[f64], window: usize) -> Option<f64> {
    if window == 0 || values.len() < window {
        return None;
    }
    let slice = &values[values.len() - window..];
    if slice.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some(slice.iter().sum::<f64>() / window as f64)
}

/// 计算末端相对 `periods` 个交易日前的收益率。
pub fn momentum(values: &[f64], periods: usize) -> Option<f64> {
    if periods == 0 || values.len() <= periods {
        return None;
    }
    let current = *values.last()?;
    let previous = values[values.len() - periods - 1];
    if !current.is_finite() || !previous.is_finite() || previous <= 0.0 {
        return None;
    }
    Some(current / previous - 1.0)
}

/// 计算最近窗口成交额与此前窗口成交额的均值比。
pub fn volume_ratio(amounts: &[f64], recent_window: usize, previous_window: usize) -> Option<f64> {
    if recent_window == 0 || previous_window == 0 || amounts.len() < recent_window + previous_window
    {
        return None;
    }
    let split = amounts.len() - recent_window;
    let previous = &amounts[split - previous_window..split];
    let recent = &amounts[split..];
    if previous
        .iter()
        .chain(recent.iter())
        .any(|value| !value.is_finite())
    {
        return None;
    }
    let previous_mean = previous.iter().sum::<f64>() / previous_window as f64;
    let recent_mean = recent.iter().sum::<f64>() / recent_window as f64;
    if previous_mean <= 0.0 {
        return None;
    }
    Some(recent_mean / previous_mean)
}

/// 计算值在样本中的经验分位。
///
/// 使用 mid-rank：小于值的样本数，加上相等样本数的一半，再除以有效样本数。
/// 这样最小值不会被强行映射为 0，重复值也不会依赖输入顺序。
pub fn percentile_rank(value: f64, samples: &[f64]) -> Option<f64> {
    if !value.is_finite() {
        return None;
    }
    let valid = samples
        .iter()
        .copied()
        .filter(|sample| sample.is_finite())
        .collect::<Vec<_>>();
    if valid.is_empty() {
        return None;
    }
    let less = valid.iter().filter(|sample| **sample < value).count() as f64;
    let equal = valid.iter().filter(|sample| **sample == value).count() as f64;
    Some((less + equal / 2.0) / valid.len() as f64)
}

/// 取最近四个报告期的净利润和，作为 TTM。
pub fn trailing_twelve_months(values: &[f64]) -> Option<f64> {
    if values.len() < 4 {
        return None;
    }
    let latest = &values[values.len() - 4..];
    if latest.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some(latest.iter().sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_window_metrics() {
        assert_eq!(simple_moving_average(&[1.0, 2.0, 3.0], 2), Some(2.5));
        assert!((momentum(&[10.0, 11.0, 12.0], 2).unwrap() - 0.2).abs() < 1e-12);
        assert_eq!(
            volume_ratio(&[10.0, 10.0, 10.0, 20.0, 20.0], 2, 3),
            Some(2.0)
        );
    }

    #[test]
    fn handles_invalid_windows_and_percentiles() {
        assert_eq!(simple_moving_average(&[1.0], 2), None);
        assert_eq!(momentum(&[0.0, 1.0], 1), None);
        assert_eq!(percentile_rank(2.0, &[1.0, 2.0, 3.0]), Some(0.5));
        assert_eq!(percentile_rank(f64::NAN, &[1.0, 2.0]), None);
    }

    #[test]
    fn calculates_ttm_from_latest_four_periods() {
        assert_eq!(
            trailing_twelve_months(&[1.0, 2.0, 3.0, 4.0, 5.0]),
            Some(14.0)
        );
        assert_eq!(trailing_twelve_months(&[1.0, 2.0, 3.0]), None);
    }
}
