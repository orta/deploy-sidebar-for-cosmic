// SPDX-License-Identifier: GPL-3.0

//! Judging a metric against its pre-deploy baseline, and formatting its values.

use crate::config::{Direction, Metric, Unit};
use crate::grafana::Labelled;
use std::collections::BTreeMap;

/// How far past the baseline (in the bad direction) counts as a warning.
const WARN_RATIO: f64 = 1.25;
/// How far past the baseline counts as a problem worth keeping the sidebar up for.
const BAD_RATIO: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Unknown,
    Ok,
    Warn,
    Bad,
}

/// Compare the current value to the baseline, in the direction the metric is bad in.
pub fn assess(metric: &Metric, baseline: Option<f64>, current: Option<f64>) -> Level {
    let (Some(baseline), Some(current)) = (baseline, current) else {
        return Level::Unknown;
    };

    let ratio = match metric.worse {
        Direction::Higher => {
            if current <= metric.floor {
                return Level::Ok;
            }
            current / baseline.max(metric.floor).max(f64::MIN_POSITIVE)
        }
        Direction::Lower => {
            // Too little traffic before the deploy to say a drop means anything.
            if baseline <= metric.floor {
                return Level::Ok;
            }
            baseline / current.max(f64::MIN_POSITIVE)
        }
    };

    if ratio >= BAD_RATIO {
        Level::Bad
    } else if ratio >= WARN_RATIO {
        Level::Warn
    } else {
        Level::Ok
    }
}

/// The mean of a series, or `None` when it is empty.
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

/// Format a value for display in its unit.
pub fn format(unit: Unit, value: f64) -> String {
    match unit {
        Unit::Count => compact(value),
        Unit::PerSecond => format!("{}/s", compact(value)),
        Unit::Ratio => format!("{:.1}%", value * 100.0),
        Unit::Seconds if value < 1.0 => format!("{:.0}ms", value * 1000.0),
        Unit::Seconds => format!("{value:.2}s"),
        Unit::Bytes => {
            let mb = value / (1024.0 * 1024.0);
            if mb >= 1024.0 {
                format!("{:.1}GB", mb / 1024.0)
            } else {
                format!("{mb:.0}MB")
            }
        }
    }
}

/// The current value relative to the baseline, e.g. "+12%".
pub fn delta(baseline: f64, current: f64) -> Option<String> {
    if baseline.abs() < f64::EPSILON {
        return None;
    }
    let change = (current - baseline) / baseline * 100.0;
    Some(format!("{change:+.0}%"))
}

/// One series of a breakdown that got worse, e.g. a route whose latency rose.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesChange {
    pub name: String,
    /// Absent when the series had no data before the deploy.
    pub baseline: Option<f64>,
    pub current: f64,
    pub level: Level,
}

/// The series of a breakdown that look worse than their own baseline, worst first.
///
/// A series missing before the deploy counts as starting from zero, so a route that only began
/// failing afterwards still shows. One missing now counts as zero, so a route whose traffic
/// stopped shows for a metric where lower is worse.
pub fn worst_series(
    metric: &Metric,
    baseline: &Labelled,
    current: &Labelled,
    limit: usize,
) -> Vec<SeriesChange> {
    let means: BTreeMap<&str, f64> = baseline
        .iter()
        .filter_map(|(name, points)| {
            let values: Vec<f64> = points.iter().map(|(_, v)| *v).collect();
            mean(&values).map(|m| (name.as_str(), m))
        })
        .collect();
    let mut latest: BTreeMap<&str, f64> = current
        .iter()
        .filter_map(|(name, points)| points.last().map(|(_, v)| (name.as_str(), *v)))
        .collect();
    if metric.worse == Direction::Lower {
        for name in means.keys() {
            latest.entry(name).or_insert(0.0);
        }
    }

    let mut changes: Vec<(f64, SeriesChange)> = latest
        .into_iter()
        .filter_map(|(name, current)| {
            let baseline = means.get(name).copied();
            let level = assess(metric, Some(baseline.unwrap_or(0.0)), Some(current));
            let b = baseline.unwrap_or(0.0);
            let severity = match metric.worse {
                Direction::Higher => current / b.max(metric.floor).max(f64::MIN_POSITIVE),
                Direction::Lower => b / current.max(f64::MIN_POSITIVE),
            };
            (level >= Level::Warn).then(|| {
                (
                    severity,
                    SeriesChange {
                        name: name.to_string(),
                        baseline,
                        current,
                        level,
                    },
                )
            })
        })
        .collect();
    changes.sort_by(|a, b| b.1.level.cmp(&a.1.level).then(b.0.total_cmp(&a.0)));
    changes.into_iter().take(limit).map(|(_, c)| c).collect()
}

fn compact(value: f64) -> String {
    if value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else if value >= 10.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(worse: Direction, floor: f64) -> Metric {
        Metric {
            label: String::new(),
            expr: String::new(),
            unit: Unit::Seconds,
            worse,
            floor,
            breakdown: None,
        }
    }

    fn series(name: &str, values: &[f64]) -> (String, Vec<(chrono::DateTime<chrono::Utc>, f64)>) {
        let points = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let t = chrono::DateTime::from_timestamp(i64::try_from(i).unwrap() * 60, 0);
                (t.unwrap(), *v)
            })
            .collect();
        (name.to_string(), points)
    }

    #[test]
    fn worst_series_ranks_regressions_and_new_failures() {
        let m = metric(Direction::Higher, 0.05);
        let baseline = vec![
            series("GET /fine", &[0.1, 0.1]),
            series("POST /graphql", &[0.2, 0.2]),
            series("GET /slower", &[0.1, 0.1]),
        ];
        let current = vec![
            series("GET /fine", &[0.1, 0.11]),
            series("POST /graphql", &[0.3, 0.5]),
            series("GET /slower", &[0.1, 0.13]),
            series("POST /new", &[0.4]),
        ];
        let worst = worst_series(&m, &baseline, &current, 2);
        let names: Vec<&str> = worst.iter().map(|c| c.name.as_str()).collect();
        // Both are bad; /new is 8x the floor, /graphql 2.5x its baseline.
        assert_eq!(names, ["POST /new", "POST /graphql"]);
        assert_eq!(worst[0].baseline, None);
        assert_eq!(worst_series(&m, &baseline, &current, 5).len(), 3);
    }

    #[test]
    fn worst_series_notices_vanished_traffic() {
        let m = metric(Direction::Lower, 1.0);
        let baseline = vec![series("GET /gone", &[50.0]), series("GET /ok", &[50.0])];
        let current = vec![series("GET /ok", &[49.0])];
        let worst = worst_series(&m, &baseline, &current, 3);
        assert_eq!(worst.len(), 1);
        assert_eq!(worst[0].name, "GET /gone");
        assert!(worst[0].current.abs() < f64::EPSILON);
    }

    #[test]
    fn higher_is_worse() {
        let m = metric(Direction::Higher, 0.0);
        assert_eq!(assess(&m, Some(1.0), Some(1.1)), Level::Ok);
        assert_eq!(assess(&m, Some(1.0), Some(1.3)), Level::Warn);
        assert_eq!(assess(&m, Some(1.0), Some(2.0)), Level::Bad);
        assert_eq!(assess(&m, None, Some(2.0)), Level::Unknown);
    }

    #[test]
    fn floor_stops_tiny_values_tripling() {
        let m = metric(Direction::Higher, 0.05);
        assert_eq!(assess(&m, Some(0.001), Some(0.003)), Level::Ok);
        // Against a zero baseline the floor stands in, so 0.1 is double it.
        assert_eq!(assess(&m, Some(0.0), Some(0.1)), Level::Bad);
    }

    #[test]
    fn lower_is_worse() {
        let m = metric(Direction::Lower, 1.0);
        assert_eq!(assess(&m, Some(100.0), Some(95.0)), Level::Ok);
        assert_eq!(assess(&m, Some(100.0), Some(50.0)), Level::Bad);
        assert_eq!(assess(&m, Some(100.0), Some(0.0)), Level::Bad);
        assert_eq!(assess(&m, Some(0.5), Some(0.0)), Level::Ok);
    }

    #[test]
    fn formatting() {
        assert_eq!(format(Unit::Seconds, 0.123), "123ms");
        assert_eq!(format(Unit::PerSecond, 1234.0), "1.2k/s");
        assert_eq!(format(Unit::Ratio, 0.025), "2.5%");
        assert_eq!(format(Unit::Bytes, 734_003_200.0), "700MB");
        assert_eq!(delta(100.0, 112.0).as_deref(), Some("+12%"));
    }
}
