//! Plain, pipe-friendly output. Device-provided text is escaped before display.
use crate::history::Summary;
use anyhow::{Result, bail};
use serde_json::Value;
use std::fmt::Write;

pub fn safe(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
fn metric(value: Option<f64>, unit: &str) -> String {
    value
        .filter(|n| n.is_finite())
        .map_or_else(|| "n/a".into(), |n| format!("{n:.2}{unit}"))
}
fn text(value: &Value) -> String {
    if value.is_null() {
        "n/a".into()
    } else {
        safe(
            value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned)
                .as_str(),
        )
    }
}
pub fn status(v: &Value) -> String {
    let state = if v.get("outage").is_some_and(|o| !o.is_null()) {
        format!(
            "Dish reports outage (cause {})",
            text(&v["outage"]["cause"])
        )
    } else {
        "No current outage reported (not an end-to-end connectivity test)".into()
    };
    let mut out = format!(
        "Starlink status\n  {state}\n  Uptime: {} s\n  Firmware: {}\n  Latency: {}\n  Packet loss: {}\n  Download / upload: {} / {}\n",
        text(&v["device_state"]["uptime_s"]),
        text(&v["device_info"]["software_version"]),
        metric(v["pop_ping_latency_ms"].as_f64(), " ms"),
        metric(v["pop_ping_drop_rate"].as_f64().map(|n| n * 100.), "%"),
        metric(
            v["downlink_throughput_bps"].as_f64().map(|n| n / 1e6),
            " Mbps"
        ),
        metric(
            v["uplink_throughput_bps"].as_f64().map(|n| n / 1e6),
            " Mbps"
        )
    );
    if let Some(obs) = v.get("obstruction_stats").filter(|x| !x.is_null()) {
        let _ = writeln!(
            out,
            "  Obstructed fraction: {}; currently obstructed: {}",
            metric(obs["fraction_obstructed"].as_f64().map(|n| n * 100.), "%"),
            text(&obs["currently_obstructed"])
        );
    }
    if let Some(gps) = v.get("gps_stats").filter(|x| !x.is_null()) {
        let _ = writeln!(
            out,
            "  GPS valid: {}; satellites: {}",
            text(&gps["gps_valid"]),
            text(&gps["gps_sats"])
        );
    }
    let alerts = v["alerts"].as_object().map(|o| {
        o.iter()
            .filter(|(_, v)| v.as_bool() == Some(true))
            .map(|(k, _)| safe(k))
            .collect::<Vec<_>>()
    });
    let _ = write!(
        out,
        "  Known active alerts: {}",
        alerts.map_or_else(
            || "not supplied".into(),
            |a| if a.is_empty() {
                "none (partial schema)".into()
            } else {
                a.join(", ")
            }
        )
    );
    out
}
pub fn summary(s: &Summary) -> String {
    format!(
        "Retained statistics: {} / {} available one-second samples\n  Mean packet loss: {}\n  Complete-loss samples: {} s in {} runs\n  Latency mean / p95: {} / {}\n  Download mean / peak: {} / {}\n  Upload mean / peak: {} / {}\n  Average power: {}\n  Explicit outage records: {} (independent of sample window)",
        s.sample_count,
        s.available_count,
        metric(s.mean_packet_loss.map(|n| n * 100.), "%"),
        s.full_loss_sample_seconds,
        s.consecutive_full_loss_runs.len(),
        metric(s.mean_latency_ms, " ms"),
        metric(s.p95_latency_ms, " ms"),
        metric(s.mean_throughput_mbps, " Mbps"),
        metric(s.max_throughput_mbps, " Mbps"),
        metric(s.mean_upload_throughput_mbps, " Mbps"),
        metric(s.max_upload_throughput_mbps, " Mbps"),
        metric(s.power_average_watts, " W"),
        s.outages.len()
    )
}
pub fn interruptions(s: &Summary, min_duration: f64) -> String {
    let mut out = summary(s);
    out.push_str("\n\nDish-reported events (oldest first; raw device timestamps, not wall-clock):\n  Start timestamp ns      Duration       Cause\n");
    let mut count = 0;
    for event in s
        .outages
        .iter()
        .filter(|o| o.ongoing || o.duration_seconds >= min_duration)
    {
        count += 1;
        let duration = if event.ongoing {
            "0s/ongoing?".into()
        } else {
            format!("{:.3}s", event.duration_seconds)
        };
        let _ = writeln!(
            out,
            "  {:<23} {:<14} {}{}",
            event.start_timestamp_ns,
            duration,
            safe(&event.cause),
            if event.did_switch { " (switched)" } else { "" }
        );
    }
    if count == 0 {
        out.push_str("  No retained events match the filter.\n");
    }
    out.push_str("\nSample-derived full-loss runs (offset 0 = oldest selected sample):\n");
    for run in &s.consecutive_full_loss_runs {
        let _ = writeln!(
            out,
            "  +{}s: {}s complete loss",
            run.start_offset, run.duration_seconds
        );
    }
    if s.consecutive_full_loss_runs.is_empty() {
        out.push_str("  No complete-loss runs in selected samples.\n");
    }
    out.push_str("Runs touching window edges may be truncated. Sub-second/partial loss is not an explicit event.\nZero-duration events may be ongoing; absence of retained events does not imply no past outages.");
    out
}
pub fn fields(title: &str, value: &Value) -> String {
    fn flatten(prefix: &str, value: &Value, out: &mut String) {
        if let Some(obj) = value.as_object() {
            for (key, value) in obj {
                let path = if prefix.is_empty() {
                    safe(key)
                } else {
                    format!("{prefix}.{}", safe(key))
                };
                flatten(&path, value, out);
            }
        } else {
            let _ = writeln!(out, "  {prefix}: {}", text(value));
        }
    }
    let mut out = format!("{title}\n");
    flatten("", value, &mut out);
    out.trim_end().to_owned()
}
pub fn clients(v: &Value) -> String {
    let mut out = String::from("Wi-Fi clients (partial schema; identifiers are private)\n");
    if let Some(clients) = v["clients"].as_array() {
        for client in clients {
            let _ = writeln!(
                out,
                "  {}  IP={}  MAC={}  active={}  signal={}",
                text(&client["name"]),
                text(&client["ip_address"]),
                text(&client["mac_address"]),
                text(&client["active"]),
                text(&client["signal_strength"])
            );
        }
        let _ = write!(out, "{} retained client entries", clients.len());
    } else {
        out.push_str("Client data not supplied.");
    }
    out
}
pub fn obstructions(v: &Value) -> Result<String> {
    let rows = v["num_rows"].as_u64().unwrap_or(0);
    let cols = v["num_cols"].as_u64().unwrap_or(0);
    let samples = v["snr"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("obstruction SNR array not supplied"))?;
    if rows.checked_mul(cols) != Some(samples.len() as u64) {
        bail!("obstruction grid dimensions do not match SNR array");
    }
    let known = samples
        .iter()
        .filter_map(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.)
        .count();
    let mut out = format!(
        "Obstruction SNR grid: {rows} rows x {cols} columns\n  Measured cells: {known} / {}\n  Reference frame: {}\n",
        samples.len(),
        text(&v["map_reference_frame"])
    );
    if rows == 0 || cols == 0 {
        out.push_str("No grid retained yet.");
        return Ok(out);
    }
    // Downsample by area; max 64x24. Not a geometrically projected sky map.
    let w = cols.min(64) as usize;
    let h = rows.min(24) as usize;
    let levels = ['.', ':', '-', '=', '+', '*', '#', '%', '@'];
    let range = samples
        .iter()
        .filter_map(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.)
        .fold(None, |range, n| {
            Some(range.map_or((n, n), |(a, b): (f64, f64)| (a.min(n), b.max(n))))
        });
    for y in 0..h {
        out.push_str("  ");
        for x in 0..w {
            let mut sum = 0.;
            let mut count = 0;
            for r in y * rows as usize / h..(y + 1) * rows as usize / h {
                for c in x * cols as usize / w..(x + 1) * cols as usize / w {
                    if let Some(n) = samples[r * cols as usize + c]
                        .as_f64()
                        .filter(|n| n.is_finite() && *n >= 0.)
                    {
                        sum += n;
                        count += 1;
                    }
                }
            }
            let ch = match (count, range) {
                (0, _) | (_, None) => ' ',
                (_, Some((low, high))) => {
                    let scaled = if high > low {
                        ((sum / count as f64 - low) / (high - low) * 8.).clamp(0., 8.)
                    } else {
                        4.
                    };
                    levels[scaled.round() as usize]
                }
            };
            out.push(ch);
        }
        out.push('\n');
    }
    out.push_str("Relative SNR: . low -> @ high; blank = unmeasured. Area-averaged grid, NOT a sky projection or obstruction classification. Use --json for full data.");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn escapes_terminal_controls_and_bidi() {
        assert_eq!(safe("hi\x1b[2J\n"), "hi\\u{1b}[2J\\n");
        assert!(!safe("\u{202e}name").contains('\u{202e}'));
    }
    #[test]
    fn unknown_telemetry_is_not_zero_or_online() {
        let out = status(&json!({}));
        assert!(out.contains("Latency: n/a"));
        assert!(out.contains("alerts: not supplied"));
        assert!(!out.contains("Online"));
    }
    #[test]
    fn grid_handles_empty_unmeasured_and_bad_dimensions() {
        assert!(
            obstructions(&json!({"num_rows":0,"num_cols":0,"snr":[]}))
                .unwrap()
                .contains("No grid")
        );
        assert!(
            obstructions(&json!({"num_rows":2,"num_cols":2,"snr":[-1,null,0,1]}))
                .unwrap()
                .contains("Measured cells: 2 / 4")
        );
        assert!(obstructions(&json!({"num_rows":2,"num_cols":2,"snr":[0]})).is_err());
        assert!(obstructions(&json!({"num_rows":u64::MAX,"num_cols":2,"snr":[]})).is_err());
    }
    #[test]
    fn interruptions_label_empty_history_honestly() {
        let s =
            crate::history::analyze(&json!({"current":0,"pop_ping_drop_rate":[]}), None).unwrap();
        assert!(interruptions(&s, 0.).contains("absence of retained events does not imply"));
    }
}
