//! Pure dashboard snapshot construction and Ratatui rendering.
//!
//! This module owns neither terminal setup nor polling.  A [`Snapshot`] is a
//! complete replacement for one history reply: callers must replace (rather
//! than append to) it when a poll reports the same `current` counter.

use anyhow::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols,
    text::Span,
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph},
};
use serde_json::Value;
use std::time::Duration;

/// One normalized chart point. `None` represents unavailable telemetry, never
/// a zero reading.  The x coordinate is seconds relative to the newest sample.
pub type Series = Vec<Option<(f64, f64)>>;

/// A validated, chronologically ordered view of one device history response.
///
/// `current` is the device's next-write counter, not a wall-clock time.
/// All series have `summary.sample_count` positions and are newest-window
/// samples in oldest-to-newest order.  `loss_percent` is always present for a
/// retained sample; optional metric series use `None` for absent, invalid, or
/// (for latency) complete-loss readings.  `status` is retained for displaying
/// a small, non-identifying status/alert summary. `summary.outages` remains
/// independent of the selected sample window.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub current: u64,
    pub status: Value,
    pub summary: crate::history::Summary,
    pub loss_percent: Series,
    pub latency_ms: Series,
    pub downlink_mbps: Series,
    pub uplink_mbps: Series,
    pub power_watts: Series,
}

/// Validate `history`, rotate its one-second ring, and retain at most its
/// newest `window` samples.  Calling `history::analyze` first deliberately
/// makes this model share the CLI's input validation rules.
pub fn snapshot(status: Value, history: &Value, window: usize) -> Result<Snapshot> {
    let summary = crate::history::analyze(history, Some(window))?;
    let decoded: crate::history::History = serde_json::from_value(history.clone())?;
    let capacity = decoded.pop_ping_drop_rate.len();
    let valid = usize::try_from(decoded.current.min(capacity as u64))?;
    let take = summary.sample_count;
    let skip = valid - take;
    let loss = rotate(&decoded.pop_ping_drop_rate, decoded.current, valid, skip);
    let latency = rotate_optional(&decoded.pop_ping_latency_ms, decoded.current, valid, skip);
    let down = rotate_optional(
        &decoded.downlink_throughput_bps,
        decoded.current,
        valid,
        skip,
    );
    let up = rotate_optional(&decoded.uplink_throughput_bps, decoded.current, valid, skip);
    let power = rotate_optional(&decoded.power_in, decoded.current, valid, skip);
    let xs = |i: usize| i as f64 - take.saturating_sub(1) as f64;
    Ok(Snapshot {
        current: decoded.current,
        status,
        summary,
        loss_percent: loss
            .iter()
            .enumerate()
            .map(|(i, value)| Some((xs(i), value * 100.0)))
            .collect(),
        latency_ms: optional_series(&latency, &loss, xs, |v| v, true),
        downlink_mbps: optional_series(&down, &loss, xs, |v| v / 1_000_000.0, false),
        uplink_mbps: optional_series(&up, &loss, xs, |v| v / 1_000_000.0, false),
        power_watts: optional_series(&power, &loss, xs, |v| v, false),
    })
}

fn rotate<T: Copy>(values: &[T], current: u64, valid: usize, skip: usize) -> Vec<T> {
    if valid == 0 {
        return Vec::new();
    }
    let start = if current >= values.len() as u64 {
        (current % values.len() as u64) as usize
    } else {
        0
    };
    (0..valid)
        .map(|i| values[(start + i) % values.len()])
        .skip(skip)
        .collect()
}
fn rotate_optional(
    values: &[Option<f64>],
    current: u64,
    valid: usize,
    skip: usize,
) -> Vec<Option<f64>> {
    if values.is_empty() {
        Vec::new()
    } else {
        rotate(values, current, valid, skip)
    }
}
fn optional_series(
    values: &[Option<f64>],
    loss: &[f64],
    x: impl Fn(usize) -> f64,
    scale: impl Fn(f64) -> f64,
    hide_full_loss: bool,
) -> Series {
    if values.is_empty() {
        return vec![None; loss.len()];
    }
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            value
                .filter(|v| v.is_finite() && *v >= 0.0)
                .filter(|_| !hide_full_loss || loss[i] < 1.0)
                .map(|v| (x(i), scale(v)))
        })
        .collect()
}

struct ChartDef<'a> {
    title: String,
    primary: &'a Series,
    color: Color,
    secondary: Option<(&'a Series, Color)>,
    fixed_loss: bool,
}

/// Draw the full-screen dashboard. `age` is elapsed time since the chart last
/// advanced, never a device timestamp. `error` means unavailable telemetry,
/// not a Starlink outage.
pub fn draw(
    frame: &mut Frame,
    snapshot: Option<&Snapshot>,
    age: Option<Duration>,
    error: Option<&str>,
    notice: Option<&str>,
    window: usize,
) {
    let area = frame.area();
    if area.width < 60 || area.height < 20 {
        let health = health_line(age, error);
        let notice = notice.map(crate::render::safe).unwrap_or_default();
        frame.render_widget(Paragraph::new(format!(
            "{health}\nDashboard needs at least 60x20 terminal cells.\n{notice}\nq / Esc / Ctrl-C: quit"
        )), area);
        return;
    }

    // These three lines are deliberately borderless and fixed: health/error,
    // summary, and quit/help (with the notice).  Alerts and events cannot push
    // an unavailable/stale indication out of view.
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    frame.render_widget(
        Paragraph::new(header(snapshot, age, error, notice, window)),
        sections[0],
    );

    let Some(s) = snapshot else {
        frame.render_widget(
            Paragraph::new("Telemetry unavailable; waiting for a successful poll.")
                .block(Block::default().borders(Borders::ALL)),
            sections[1],
        );
        return;
    };

    let event_rows = match sections[1].height {
        h if h >= 25 => 4, // label plus three explicit events
        h if h >= 18 => 3, // label plus two
        _ => 2,            // label plus newest event
    };
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(event_rows)])
        .split(sections[1]);
    render_charts(frame, body[0], s, window);
    render_events(frame, body[1], s, event_rows.saturating_sub(1) as usize);
}

fn health_line(age: Option<Duration>, error: Option<&str>) -> String {
    if let Some(error) = error {
        format!(
            "TELEMETRY UNAVAILABLE (chart age {}): {}",
            age.map_or_else(|| "n/a".into(), |d| format!("{}s", d.as_secs())),
            crate::render::safe(error)
        )
    } else if let Some(age) = age {
        if age > Duration::from_secs(5) {
            format!("STALE: chart last advanced {}s ago", age.as_secs())
        } else {
            format!("LIVE: chart last advanced {}s ago", age.as_secs())
        }
    } else {
        "WAITING: no chart sample yet".into()
    }
}

fn header(
    s: Option<&Snapshot>,
    age: Option<Duration>,
    error: Option<&str>,
    notice: Option<&str>,
    window: usize,
) -> String {
    let summary = s.map_or_else(
        || "Summary: telemetry not yet available".into(),
        |s| {
            format!(
                "{} / {} samples | loss {} | latency {} | window {}s",
                s.summary.sample_count,
                s.summary.available_count,
                metric(s.summary.mean_packet_loss.map(|v| v * 100.0), "%"),
                metric(s.summary.mean_latency_ms, " ms"),
                window
            )
        },
    );
    let notice = notice
        .map(crate::render::safe)
        .unwrap_or_else(|| "x: seconds before newest sample (0); not wall-clock".into());
    format!(
        "{}\n{}\nq / Esc / Ctrl-C: quit | Notice: {}",
        health_line(age, error),
        summary,
        notice
    )
}

fn render_charts(frame: &mut Frame, area: Rect, s: &Snapshot, window: usize) {
    let charts = if area.width >= 100 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area)
            .iter()
            .flat_map(|column| {
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(*column)
                    .to_vec()
            })
            .collect::<Vec<_>>()
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(34),
                Constraint::Percentage(33),
                Constraint::Percentage(33),
            ])
            .split(area)
            .to_vec()
    };
    let defs = vec![
        ChartDef {
            title: titled("Latency (ms)", &s.latency_ms),
            primary: &s.latency_ms,
            color: Color::Cyan,
            secondary: None,
            fixed_loss: false,
        },
        ChartDef {
            title: titled("Packet loss (%)", &s.loss_percent),
            primary: &s.loss_percent,
            color: Color::Red,
            secondary: None,
            fixed_loss: true,
        },
        ChartDef {
            title: throughput_title(&s.downlink_mbps, &s.uplink_mbps),
            primary: &s.downlink_mbps,
            color: Color::Cyan,
            secondary: Some((&s.uplink_mbps, Color::Yellow)),
            fixed_loss: false,
        },
        ChartDef {
            title: titled("Power (W)", &s.power_watts),
            primary: &s.power_watts,
            color: Color::Green,
            secondary: None,
            fixed_loss: false,
        },
    ];
    for (area, def) in charts.into_iter().zip(defs) {
        render_chart(frame, area, def, window);
    }
}

fn titled(name: &str, series: &Series) -> String {
    if series.iter().any(Option::is_some) {
        name.into()
    } else {
        format!("{name} — no data / n/a")
    }
}
fn throughput_title(down: &Series, up: &Series) -> String {
    let d = if down.iter().any(Option::is_some) {
        "down cyan"
    } else {
        "down n/a"
    };
    let u = if up.iter().any(Option::is_some) {
        "up yellow"
    } else {
        "up n/a"
    };
    format!("Throughput (Mbps): {d}, {u}")
}

fn render_events(frame: &mut Frame, area: Rect, s: &Snapshot, count: usize) {
    let mut lines = vec![format!(
        "Events: {} retained; raw start ns, independent of charts | {} | alerts: {}",
        s.summary.outages.len(),
        status_brief(&s.status),
        alerts(&s.status)
    )];
    let events = s
        .summary
        .outages
        .iter()
        .rev()
        .take(count)
        .collect::<Vec<_>>();
    if events.is_empty() {
        lines.push("No retained explicit events (not proof of no past outages).".into());
    }
    for event in events {
        lines.push(format!(
            "{}  {:.3}s  start_ns={}{}",
            crate::render::safe(&event.cause),
            event.duration_seconds,
            event.start_timestamp_ns,
            if event.ongoing {
                " (possibly ongoing)"
            } else {
                ""
            }
        ));
    }
    frame.render_widget(Paragraph::new(lines.join("\n")), area);
}

fn metric(value: Option<f64>, suffix: &str) -> String {
    value
        .filter(|v| v.is_finite())
        .map_or_else(|| "n/a".into(), |v| format!("{v:.2}{suffix}"))
}
fn status_brief(v: &Value) -> String {
    if let Some(outage) = v.get("outage").filter(|o| !o.is_null()) {
        format!(
            "dish reports outage {}",
            crate::render::safe(
                outage
                    .get("cause")
                    .map(Value::to_string)
                    .as_deref()
                    .unwrap_or("unknown")
            )
        )
    } else {
        "no current outage reported".into()
    }
}
fn alerts(v: &Value) -> String {
    v.get("alerts")
        .and_then(Value::as_object)
        .map(|a| {
            let names: Vec<_> = a
                .iter()
                .filter(|(_, x)| x.as_bool() == Some(true))
                .map(|(k, _)| crate::render::safe(k))
                .collect();
            if names.is_empty() {
                "none (partial schema)".into()
            } else {
                names.join(", ")
            }
        })
        .unwrap_or_else(|| "not supplied".into())
}

fn render_chart(frame: &mut Frame, area: Rect, def: ChartDef<'_>, _window: usize) {
    let points = def.primary.iter().flatten().copied().collect::<Vec<_>>();
    let second = def
        .secondary
        .map(|(s, c)| (s.iter().flatten().copied().collect::<Vec<_>>(), c));
    let raw_max = points
        .iter()
        .map(|p| p.1)
        .chain(second.iter().flat_map(|(p, _)| p.iter().map(|x| x.1)))
        .filter(|v| v.is_finite())
        .fold(0.0_f64, f64::max);
    let max = if def.fixed_loss {
        100.0
    } else if raw_max <= 1.0 {
        1.0
    } else if raw_max <= f64::MAX / 1.1 {
        raw_max * 1.1
    } else {
        raw_max
    };
    // Bounds follow retained samples, not the requested window. One sample
    // still needs a non-degenerate x range for Ratatui.
    let sample_count = def
        .primary
        .len()
        .max(def.secondary.map_or(0, |(s, _)| s.len()));
    let start = -(sample_count.saturating_sub(1).max(1) as f64);
    let mut datasets = vec![
        Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Scatter)
            .style(Style::default().fg(def.color))
            .data(&points),
    ];
    if let Some((other, color)) = &second {
        datasets.push(
            Dataset::default()
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Scatter)
                .style(Style::default().fg(*color))
                .data(other),
        );
    }
    let chart = Chart::new(datasets)
        .block(Block::default().borders(Borders::ALL).title(def.title))
        .x_axis(
            Axis::default()
                .bounds([start, 0.0])
                .labels(vec![Span::raw(format!("{start:.0}s")), Span::raw("0s")]),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, max])
                .labels(vec![Span::raw("0"), Span::raw(format!("{max:.0}"))]),
        );
    frame.render_widget(chart, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    fn history() -> Value {
        json!({"current":6,"pop_ping_drop_rate":[1.,0.,0.5,0.],"pop_ping_latency_ms":[9.,10.,11.,12.],"downlink_throughput_bps":[1e6,null,-1.,2e6]})
    }
    fn screen(t: &Terminal<TestBackend>) -> String {
        t.backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }
    #[test]
    fn model_rotates_and_hides_latency_on_loss() {
        let s = snapshot(json!({}), &history(), 3).unwrap();
        assert_eq!(
            s.loss_percent
                .iter()
                .flatten()
                .map(|p| p.1)
                .collect::<Vec<_>>(),
            vec![0., 100., 0.]
        );
        assert_eq!(s.latency_ms[1], None);
    }
    #[test]
    fn zero_window_and_missing_series_are_safe() {
        assert!(
            snapshot(json!({}), &json!({"current":0,"pop_ping_drop_rate":[]}), 0)
                .unwrap()
                .loss_percent
                .is_empty()
        );
    }
    #[test]
    fn health_and_notice_are_in_the_rendered_buffer_at_all_sizes() {
        for (w, h) in [(120, 40), (70, 25), (59, 19)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| {
                draw(
                    f,
                    Some(&snapshot(json!({}), &history(), 300).unwrap()),
                    Some(Duration::from_secs(9)),
                    Some("transport failed"),
                    Some("retrying soon"),
                    300,
                )
            })
            .unwrap();
            let output = screen(&t);
            assert!(
                output.contains("TELEMETRY UNAVAILABLE"),
                "{w}x{h}: {output}"
            );
            assert!(output.contains("retrying soon"), "{w}x{h}: {output}");
        }
    }
    #[test]
    fn missing_metrics_and_single_sample_are_labeled_and_safe() {
        let h = json!({"current":1,"pop_ping_drop_rate":[0.],"pop_ping_latency_ms":[],"downlink_throughput_bps":[],"uplink_throughput_bps":[],"power_in":[]});
        let s = snapshot(json!({}), &h, 1).unwrap();
        let mut t = Terminal::new(TestBackend::new(70, 25)).unwrap();
        t.draw(|f| draw(f, Some(&s), Some(Duration::ZERO), None, None, 1))
            .unwrap();
        let output = screen(&t);
        assert!(output.contains("Latency (ms) — no data / n/a"));
        assert!(output.contains("down n/a, up n/a"));
    }
    #[test]
    fn repeated_snapshots_do_not_duplicate_and_optional_gaps_stay_missing() {
        let h = json!({"current":5,"pop_ping_drop_rate":[0.,1.,0.],"pop_ping_latency_ms":[null,22.,30.],"downlink_throughput_bps":[1e6,-1.,null]});
        let a = snapshot(json!({}), &h, 99).unwrap();
        let b = snapshot(json!({}), &h, 99).unwrap();
        assert_eq!(a.loss_percent, b.loss_percent);
        assert_eq!(a.latency_ms, vec![Some((-2., 30.)), None, None]);
        assert_eq!(a.downlink_mbps, vec![None, Some((-1., 1.)), None]);
        assert_eq!(a.uplink_mbps, vec![None; 3]);
        assert!(
            snapshot(
                json!({}),
                &json!({"current":1,"pop_ping_drop_rate":[1.1]}),
                3
            )
            .is_err()
        );
    }
    #[test]
    fn explicit_events_sort_newest_first_and_escape_untrusted_strings() {
        let h = json!({"current":1,"pop_ping_drop_rate":[0.],"outages":[
            {"cause":6,"start_timestamp_ns":99,"duration_ns":2_000_000_000u64},
            {"cause":777,"start_timestamp_ns":1,"duration_ns":0}]});
        let s = snapshot(json!({"alerts":{"bad\u{1b}[2J":true}}), &h, 1).unwrap();
        let mut t = Terminal::new(TestBackend::new(120, 36)).unwrap();
        t.draw(|f| draw(f, Some(&s), Some(Duration::ZERO), None, None, 1))
            .unwrap();
        let output = screen(&t);
        assert!(output.find("obstructed").unwrap() < output.find("unknown(777)").unwrap());
        assert!(output.contains("2.000s"));
        assert!(!output.contains('\u{1b}'));
    }
    #[test]
    fn all_sizes_and_empty_samples_render_without_panicking() {
        let s = snapshot(
            json!({}),
            &json!({"current":0,"pop_ping_drop_rate":[]}),
            300,
        )
        .unwrap();
        for (w, h) in [
            (0, 0),
            (1, 1),
            (59, 19),
            (60, 20),
            (80, 24),
            (100, 20),
            (120, 36),
        ] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| draw(f, Some(&s), Some(Duration::from_secs(20)), None, None, 300))
                .unwrap();
            if w >= 60 && h >= 20 {
                assert!(screen(&t).contains("STALE"));
            }
        }
    }
    #[test]
    fn extreme_chart_values_keep_finite_scale() {
        let h = json!({"current":1,"pop_ping_drop_rate":[0.],"pop_ping_latency_ms":[f64::MAX]});
        let s = snapshot(json!({}), &h, 1).unwrap();
        let mut t = Terminal::new(TestBackend::new(120, 36)).unwrap();
        t.draw(|f| draw(f, Some(&s), Some(Duration::ZERO), None, None, 1))
            .unwrap();
        assert!(!screen(&t).contains("inf"));
    }
    #[test]
    fn initial_and_failure_render() {
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| draw(f, None, None, Some("down"), None, 300))
            .unwrap();
        assert!(screen(&t).contains("TELEMETRY UNAVAILABLE"));
    }
}
