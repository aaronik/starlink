//! Pure analysis of the dish's inner `get_history` JSON response.
//!
//! History arrays are one-second ring buffers. `current` is the total number of
//! samples written, and `current % capacity` is the next slot to write.  This
//! module therefore reports sample offsets relative to the selected (possibly
//! windowed) chronological data, rather than inventing wall-clock timestamps.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The decoded history input. Empty optional arrays mean that a metric was not
/// supplied by this firmware, not a sequence of zero-valued measurements.
/// Values in unwritten packet-loss slots are ignored: some firmware pads them
/// with sentinel values rather than telemetry.
#[derive(Debug, Clone, Deserialize)]
pub struct History {
    pub current: u64,
    pub pop_ping_drop_rate: Vec<f64>,
    #[serde(default)]
    pub pop_ping_latency_ms: Vec<Option<f64>>,
    #[serde(default)]
    pub downlink_throughput_bps: Vec<Option<f64>>,
    #[serde(default)]
    pub uplink_throughput_bps: Vec<Option<f64>>,
    #[serde(default)]
    pub power_in: Vec<Option<f64>>,
    #[serde(default)]
    pub outages: Vec<Outage>,
}

/// A dish-reported outage. `duration_ns == 0` means a zero-duration outage,
/// which may still be ongoing; the protocol does not distinguish those cases.
#[derive(Debug, Clone, Deserialize)]
pub struct Outage {
    pub cause: i64,
    pub start_timestamp_ns: i64,
    pub duration_ns: u64,
    #[serde(default)]
    pub did_switch: bool,
}

/// A contiguous run of one-second, complete packet-loss samples.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FullLossRun {
    /// Offset in the selected chronological sample window (zero is oldest).
    pub start_offset: usize,
    pub duration_seconds: usize,
}

/// A normalized explicit outage reported by the dish.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutageRecord {
    pub cause: String,
    pub start_timestamp_ns: i64,
    pub duration_seconds: f64,
    pub did_switch: bool,
    /// True for zero duration, which is possibly ongoing but not conclusive.
    pub ongoing: bool,
}

/// Statistics over the retained history, optionally restricted to its newest
/// `sample_count` is the number of samples analyzed after applying `window`;
/// `available_count` is the number retained by the device before that window.
/// Both exclude unwritten ring-buffer slots.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub sample_count: usize,
    pub available_count: usize,
    pub mean_packet_loss: Option<f64>,
    pub full_loss_sample_seconds: usize,
    pub mean_latency_ms: Option<f64>,
    pub p95_latency_ms: Option<f64>,
    /// Download throughput statistics retained for CLI compatibility.
    pub mean_throughput_mbps: Option<f64>,
    pub max_throughput_mbps: Option<f64>,
    pub mean_download_throughput_mbps: Option<f64>,
    pub max_download_throughput_mbps: Option<f64>,
    pub mean_upload_throughput_mbps: Option<f64>,
    pub max_upload_throughput_mbps: Option<f64>,
    pub power_average_watts: Option<f64>,
    pub consecutive_full_loss_runs: Vec<FullLossRun>,
    pub outages: Vec<OutageRecord>,
}

/// Analyze an inner history response. Required loss values must be finite and
/// in `[0, 1]`. `null` optional metric values and non-finite optional numeric
/// values are ignored rather than treated as zero. JSON strings (including
/// `"NaN"`) are rejected. All nonempty metric arrays must have the loss array's
/// capacity.
pub fn analyze(value: &Value, window: Option<usize>) -> Result<Summary> {
    let history: History = serde_json::from_value(value.clone()).context("invalid history JSON")?;
    let capacity = history.pop_ping_drop_rate.len();
    if capacity == 0 && history.current != 0 {
        bail!("history current is nonzero but packet-loss capacity is zero");
    }
    for (name, series) in [
        ("pop_ping_latency_ms", &history.pop_ping_latency_ms),
        ("downlink_throughput_bps", &history.downlink_throughput_bps),
        ("uplink_throughput_bps", &history.uplink_throughput_bps),
        ("power_in", &history.power_in),
    ] {
        if !series.is_empty() && series.len() != capacity {
            bail!("history {name} length does not match packet-loss capacity");
        }
    }
    let valid = usize::try_from(history.current.min(capacity as u64))
        .context("history sample count does not fit this platform")?;
    for loss in rotate(&history.pop_ping_drop_rate, history.current, valid, 0) {
        if !loss.is_finite() || !(0.0..=1.0).contains(&loss) {
            bail!("packet-loss values must be finite and in [0, 1]");
        }
    }
    let selected = window.map_or(valid, |n| n.min(valid));
    let skip = valid - selected;
    let loss = rotate(&history.pop_ping_drop_rate, history.current, valid, skip);
    let latency = rotate_optional(&history.pop_ping_latency_ms, history.current, valid, skip);
    let download_throughput = rotate_optional(
        &history.downlink_throughput_bps,
        history.current,
        valid,
        skip,
    );
    let upload_throughput =
        rotate_optional(&history.uplink_throughput_bps, history.current, valid, skip);
    let power = rotate_optional(&history.power_in, history.current, valid, skip);

    let full: Vec<bool> = loss.iter().map(|v| *v >= 1.0).collect();
    let latency_values: Vec<f64> = latency
        .iter()
        .zip(&full)
        .filter_map(|(v, full)| match (*v, *full) {
            (Some(value), false) if value.is_finite() && value >= 0.0 => Some(value),
            _ => None,
        })
        .collect();
    let download_throughput_values = throughput_mbps(&download_throughput);
    let upload_throughput_values = throughput_mbps(&upload_throughput);
    let power_values: Vec<f64> = power
        .iter()
        .filter_map(|v| v.filter(|v| v.is_finite() && *v >= 0.0))
        .collect();

    let mut sorted_latency = latency_values.clone();
    sorted_latency.sort_by(f64::total_cmp);
    let p95 = (!sorted_latency.is_empty()).then(|| {
        // Nearest-rank p95: ceil(0.95*n), using one-based ranks.
        sorted_latency[((sorted_latency.len() * 95).div_ceil(100)).saturating_sub(1)]
    });
    let mut outages: Vec<OutageRecord> = history
        .outages
        .iter()
        .map(normalize_outage)
        .collect::<Result<_>>()?;
    outages.sort_by_key(|o| o.start_timestamp_ns);

    Ok(Summary {
        sample_count: selected,
        available_count: valid,
        mean_packet_loss: mean(&loss),
        full_loss_sample_seconds: full.iter().filter(|v| **v).count(),
        mean_latency_ms: mean(&latency_values),
        p95_latency_ms: p95,
        mean_throughput_mbps: mean(&download_throughput_values),
        max_throughput_mbps: download_throughput_values.iter().copied().reduce(f64::max),
        mean_download_throughput_mbps: mean(&download_throughput_values),
        max_download_throughput_mbps: download_throughput_values.iter().copied().reduce(f64::max),
        mean_upload_throughput_mbps: mean(&upload_throughput_values),
        max_upload_throughput_mbps: upload_throughput_values.iter().copied().reduce(f64::max),
        power_average_watts: mean(&power_values),
        consecutive_full_loss_runs: runs(&full),
        outages,
    })
}

fn rotate<T: Copy>(series: &[T], current: u64, valid: usize, skip: usize) -> Vec<T> {
    if valid == 0 {
        return Vec::new();
    }
    let capacity = series.len();
    let start = if current >= capacity as u64 {
        (current % capacity as u64) as usize
    } else {
        0
    };
    (0..valid)
        .map(|i| series[(start + i) % capacity])
        .skip(skip)
        .collect()
}

fn rotate_optional<T: Copy>(series: &[T], current: u64, valid: usize, skip: usize) -> Vec<T> {
    if series.is_empty() {
        Vec::new()
    } else {
        rotate(series, current, valid, skip)
    }
}

fn throughput_mbps(values: &[Option<f64>]) -> Vec<f64> {
    values
        .iter()
        .filter_map(|value| value.filter(|value| value.is_finite() && *value >= 0.0))
        .map(|value| value / 1_000_000.0)
        .collect()
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    // Incremental averaging avoids overflowing a finite sum of large counters.
    let mut average = 0.0;
    for (index, value) in values.iter().copied().enumerate() {
        average += (value - average) / (index + 1) as f64;
    }
    Some(average)
}

fn runs(full: &[bool]) -> Vec<FullLossRun> {
    let mut result = Vec::new();
    let mut start = None;
    for (offset, is_full) in full.iter().copied().enumerate() {
        match (start, is_full) {
            (None, true) => start = Some(offset),
            (Some(first), false) => {
                result.push(FullLossRun {
                    start_offset: first,
                    duration_seconds: offset - first,
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(first) = start {
        result.push(FullLossRun {
            start_offset: first,
            duration_seconds: full.len() - first,
        });
    }
    result
}

fn normalize_outage(outage: &Outage) -> Result<OutageRecord> {
    let duration_seconds = outage.duration_ns as f64 / 1_000_000_000.0;
    Ok(OutageRecord {
        cause: outage_cause_name(outage.cause).to_owned(),
        start_timestamp_ns: outage.start_timestamp_ns,
        duration_seconds,
        did_switch: outage.did_switch,
        // Zero duration is ambiguous in the device protocol; retain this
        // compatibility indicator but do not claim that it proves an outage is live.
        ongoing: outage.duration_ns == 0,
    })
}

/// Names used by the DishOutage enum in the Starlink web UI. Unrecognized
/// future enum values remain visible as `unknown(<number>)`.
fn outage_cause_name(cause: i64) -> String {
    match cause {
        0 => "unknown".into(),
        1 => "booting".into(),
        2 => "thermal_shutdown".into(),
        3 => "thermal_throttle".into(),
        4 => "no_schedule".into(),
        5 => "no_sats".into(),
        6 => "obstructed".into(),
        7 => "no_downlink".into(),
        8 => "no_pings".into(),
        9 => "slow_downlink".into(),
        10 => "slow_uplink".into(),
        11 => "power_save".into(),
        value => format!("unknown({value})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn history(current: u64, loss: Vec<f64>) -> Value {
        json!({"current": current, "pop_ping_drop_rate": loss})
    }

    #[test]
    fn wrapping_rotates_and_reports_runs() {
        let mut v = history(6, vec![1., 0., 1., 1.]); // chronological: 1,1,1,0
        v["pop_ping_latency_ms"] = json!([4., 5., 6., 7.]);
        let s = analyze(&v, None).unwrap();
        assert_eq!(s.full_loss_sample_seconds, 3);
        assert_eq!(
            s.consecutive_full_loss_runs,
            vec![FullLossRun {
                start_offset: 0,
                duration_seconds: 3
            }]
        );
        assert_eq!(s.mean_latency_ms, Some(5.0));
    }
    #[test]
    fn current_zero_and_empty_are_valid() {
        let s = analyze(&history(0, vec![]), None).unwrap();
        assert_eq!(s.sample_count, 0);
        assert_eq!(s.mean_packet_loss, None);
    }
    #[test]
    fn reboot_before_capacity_does_not_rotate() {
        let s = analyze(&history(2, vec![0., 1., 0., 0.]), None).unwrap();
        assert_eq!(s.sample_count, 2);
        assert_eq!(s.full_loss_sample_seconds, 1);
    }
    #[test]
    fn missing_optional_metrics_are_not_zero() {
        let s = analyze(&history(1, vec![0.]), None).unwrap();
        assert_eq!(s.mean_latency_ms, None);
        assert_eq!(s.mean_throughput_mbps, None);
        assert_eq!(s.power_average_watts, None);
    }
    #[test]
    fn window_selects_newest_samples() {
        let s = analyze(&history(4, vec![0., 0., 1., 1.]), Some(2)).unwrap();
        assert_eq!(s.sample_count, 2);
        assert_eq!(s.consecutive_full_loss_runs[0].start_offset, 0);
    }
    #[test]
    fn partial_loss_keeps_latency_but_full_loss_does_not() {
        let mut v = history(3, vec![0.5, 1., 0.]);
        v["pop_ping_latency_ms"] = json!([10., 20., 30.]);
        let s = analyze(&v, None).unwrap();
        assert_eq!(s.mean_latency_ms, Some(20.));
        assert_eq!(s.p95_latency_ms, Some(30.));
    }
    #[test]
    fn latency_p95_uses_nearest_rank() {
        let mut v = history(4, vec![0.; 4]);
        v["pop_ping_latency_ms"] = json!([1., 2., 3., 4.]);
        assert_eq!(analyze(&v, None).unwrap().p95_latency_ms, Some(4.));
    }
    #[test]
    fn outages_are_sorted_named_and_ongoing() {
        let mut v = history(0, vec![]);
        v["outages"] = json!([{ "cause": 99, "start_timestamp_ns": 9, "duration_ns": 0 }, { "cause": 6, "start_timestamp_ns": 1, "duration_ns": 2_000_000_000u64 }]);
        let s = analyze(&v, None).unwrap();
        assert_eq!(s.outages[0].cause, "obstructed");
        assert!(s.outages[1].ongoing);
        assert_eq!(s.outages[1].cause, "unknown(99)");
    }
    #[test]
    fn rejects_bad_input_and_filters_optional_nan() {
        assert!(analyze(&history(1, vec![1.1]), None).is_err());
        let mut v = history(1, vec![0.]);
        v["pop_ping_latency_ms"] = json!(["NaN"]);
        assert!(analyze(&v, None).is_err());
    }
    #[test]
    fn rejects_mismatched_arrays() {
        for field in [
            "pop_ping_latency_ms",
            "downlink_throughput_bps",
            "uplink_throughput_bps",
            "power_in",
        ] {
            let mut v = history(2, vec![0., 0.]);
            v[field] = json!([1.]);
            assert!(analyze(&v, None).is_err(), "{field} mismatch accepted");
        }
    }

    #[test]
    fn null_optional_telemetry_is_ignored() {
        let mut v = history(3, vec![0., 0., 0.]);
        v["pop_ping_latency_ms"] = json!([null, 10., null]);
        v["downlink_throughput_bps"] = json!([null, 2_000_000., null]);
        v["uplink_throughput_bps"] = json!([null, 500_000., null]);
        v["power_in"] = json!([null, 40., null]);
        let s = analyze(&v, None).unwrap();
        assert_eq!(s.mean_latency_ms, Some(10.));
        assert_eq!(s.mean_download_throughput_mbps, Some(2.));
        assert_eq!(s.mean_upload_throughput_mbps, Some(0.5));
        assert_eq!(s.power_average_watts, Some(40.));
    }

    #[test]
    fn zero_capacity_and_windows_are_safe() {
        assert!(analyze(&history(1, vec![]), None).is_err());
        let v = history(2, vec![0., 1.]);
        assert_eq!(analyze(&v, Some(0)).unwrap().sample_count, 0);
        assert_eq!(analyze(&v, Some(usize::MAX)).unwrap().sample_count, 2);
    }

    #[test]
    fn ignores_unwritten_sentinel_padding_but_not_written_values() {
        let s = analyze(&history(2, vec![0., 1., -1., -1.]), None).unwrap();
        assert_eq!(s.sample_count, 2);
        assert_eq!(s.full_loss_sample_seconds, 1);
        assert!(analyze(&history(3, vec![0., 1., -1., -1.]), None).is_err());
    }

    #[test]
    fn handles_very_large_counter_and_clips_runs_at_window_boundary() {
        let mut v = history(u64::MAX, vec![1., 0., 0., 1.]);
        // u64::MAX % 4 == 3, chronological order is slots 3, 0, 1, 2.
        let s = analyze(&v, Some(2)).unwrap();
        assert!(s.consecutive_full_loss_runs.is_empty());
        v["pop_ping_drop_rate"] = json!([1., 1., 0., 0.]);
        let s = analyze(&v, None).unwrap();
        assert_eq!(
            s.consecutive_full_loss_runs,
            vec![FullLossRun {
                start_offset: 1,
                duration_seconds: 2
            }]
        );
    }

    #[test]
    fn outage_signed_values_and_known_or_unknown_causes() {
        let mut v = history(0, vec![]);
        v["outages"] = json!([
            { "cause": 10, "start_timestamp_ns": -5, "duration_ns": 0 },
            { "cause": 11, "start_timestamp_ns": 3, "duration_ns": 1_000_000_000 },
            { "cause": 42, "start_timestamp_ns": 4, "duration_ns": 1 }
        ]);
        let s = analyze(&v, None).unwrap();
        assert_eq!(s.outages[0].start_timestamp_ns, -5);
        assert_eq!(s.outages[0].cause, "slow_uplink");
        assert!(s.outages[0].ongoing);
        assert_eq!(s.outages[1].cause, "power_save");
        assert_eq!(s.outages[2].cause, "unknown(42)");
        v["outages"] = json!([{ "cause": 0, "start_timestamp_ns": 0, "duration_ns": -1 }]);
        assert!(analyze(&v, None).is_err());
    }

    #[test]
    fn transfer_units_power_and_p95_are_correct() {
        let mut v = history(20, vec![0.; 20]);
        v["pop_ping_latency_ms"] = json!((1..=20).collect::<Vec<_>>());
        v["downlink_throughput_bps"] = json!(vec![2_000_000.; 20]);
        v["uplink_throughput_bps"] = json!(vec![500_000.; 20]);
        v["power_in"] = json!(vec![42.; 20]);
        let s = analyze(&v, None).unwrap();
        assert_eq!(s.p95_latency_ms, Some(19.));
        assert_eq!(s.mean_throughput_mbps, Some(2.));
        assert_eq!(s.max_download_throughput_mbps, Some(2.));
        assert_eq!(s.mean_upload_throughput_mbps, Some(0.5));
        assert_eq!(s.max_upload_throughput_mbps, Some(0.5));
        assert_eq!(s.power_average_watts, Some(42.));
    }

    #[test]
    fn mean_does_not_overflow_for_finite_values() {
        assert_eq!(mean(&[f64::MAX, f64::MAX]), Some(f64::MAX));
    }
}
