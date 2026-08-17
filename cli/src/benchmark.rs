//! Measurement helpers used by the native benchmark command.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryV1 {
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
}

pub fn summarize(values: &[f64]) -> SummaryV1 {
    if values.is_empty() {
        return SummaryV1 {
            mean_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            min_ms: 0.0,
            max_ms: 0.0,
        };
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let last = ordered.len() - 1;
    let mean_ms = ordered.iter().sum::<f64>() / ordered.len() as f64;
    SummaryV1 {
        mean_ms: rounded(mean_ms),
        p50_ms: rounded(ordered[last / 2]),
        p95_ms: rounded(ordered[((last as f64 * 0.95).round() as usize).min(last)]),
        min_ms: rounded(ordered[0]),
        max_ms: rounded(ordered[last]),
    }
}

fn rounded(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_stable_and_bounded() {
        let result = summarize(&[10.0, 20.0, 30.0, 40.0, 50.0]);
        assert_eq!(result.mean_ms, 30.0);
        assert_eq!(result.p50_ms, 30.0);
        assert_eq!(result.p95_ms, 50.0);
    }
}
