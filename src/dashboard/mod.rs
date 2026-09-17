//! Interactive, read-only terminal dashboard. Polling never blocks input/redraw.
pub mod view;

use crate::client::{Client, Query};
use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::{
    io::{self, IsTerminal},
    time::{Duration, Instant},
};

/// Replaces entire retained snapshots rather than appending overlapping history.
#[derive(Default)]
struct State {
    snapshot: Option<view::Snapshot>,
    advanced_at: Option<Instant>,
    error: Option<String>,
    notice: Option<String>,
}
impl State {
    fn accept(&mut self, next: view::Snapshot, now: Instant) {
        let previous = self.snapshot.as_ref();
        let counter_reset = previous.is_some_and(|p| next.current < p.current);
        let uptime_reset = previous.is_some_and(|p| {
            match (
                p.status["device_state"]["uptime_s"].as_u64(),
                next.status["device_state"]["uptime_s"].as_u64(),
            ) {
                (Some(a), Some(b)) => b < a,
                _ => false,
            }
        });
        if previous.is_none_or(|p| next.current != p.current) || uptime_reset {
            self.advanced_at = Some(now);
        }
        self.notice = if counter_reset || uptime_reset {
            Some("Device counter/uptime reset; showing only new retained history.".into())
        } else if previous.is_some_and(|p| next.current == p.current) {
            Some("History counter unchanged; charts have not advanced.".into())
        } else {
            None
        };
        self.snapshot = Some(next);
        self.error = None;
    }
    fn fail(&mut self, error: anyhow::Error) {
        self.error = Some(format!("{error:#}"));
        // Keep old data with its ORIGINAL age, never mark a failed poll fresh.
    }
}

/// Always unwind the alternate screen/raw mode, including partial setup failures.
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

fn exit_key(event: &Event) -> bool {
    matches!(event, Event::Key(key) if key.kind != KeyEventKind::Release &&
        (matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc) ||
        key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)))
}

/// Requires a real terminal; use `watch --json` for redirected/piped telemetry.
pub async fn run(client: &Client, interval: Duration, window: usize) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!(
            "dashboard requires an interactive terminal on stdin and stdout; use watch or watch --json for pipes"
        );
    }
    if interval < Duration::from_secs(1) || !(1..=86400).contains(&window) {
        bail!("dashboard interval must be >=1 second and window must be 1..=86400 samples");
    }
    let _guard = TerminalGuard;
    // Ratatui installs a panic hook that restores the screen before printing a panic.
    let mut terminal = ratatui::try_init().context("initializing dashboard terminal")?;
    terminal.clear()?;
    let mut keyboard = tokio::time::interval(Duration::from_millis(50));
    keyboard.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut state = State::default();
    let mut redraw = tokio::time::interval(Duration::from_millis(200));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let terminate_signal = async {
        #[cfg(unix)]
        terminate.recv().await;
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    };
    tokio::pin!(terminate_signal);
    let mut delay = Duration::ZERO;
    loop {
        // Delay and network operations share a cancellable future. Input and redraw
        // stay responsive while connecting, reading, retrying, or sleeping.
        let poll = async move {
            tokio::time::sleep(delay).await;
            let status = client.query(Query::Status).await?;
            let history = client.query(Query::History).await?;
            view::snapshot(status, &history, window)
        };
        tokio::pin!(poll);
        loop {
            tokio::select! {
                result = &mut interrupted => { result.context("listening for Ctrl-C")?; return Ok(()); },
                _ = &mut terminate_signal => return Ok(()),
                _ = keyboard.tick() => {
                    // No EventStream worker holds the global crossterm reader lock.
                    // Consume only already-ready input, with a bound against key floods.
                    for _ in 0..32 {
                        if !event::poll(Duration::ZERO).context("polling dashboard keyboard")? { break; }
                        if exit_key(&event::read().context("reading dashboard keyboard")?) { return Ok(()); }
                    }
                },
                _ = redraw.tick() => {
                    terminal.draw(|frame| view::draw(frame, state.snapshot.as_ref(), state.advanced_at.map(|t| t.elapsed()), state.error.as_deref(), state.notice.as_deref(), window))?;
                },
                result = &mut poll => {
                    match result {
                        Ok(snapshot) => state.accept(snapshot, Instant::now()),
                        Err(error) => state.fail(error),
                    }
                    terminal.draw(|frame| view::draw(frame, state.snapshot.as_ref(), state.advanced_at.map(|t| t.elapsed()), state.error.as_deref(), state.notice.as_deref(), window))?;
                    break;
                },
            }
        }
        delay = interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn snapshot(current: u64, uptime: u64) -> view::Snapshot {
        view::snapshot(
            json!({"device_state":{"uptime_s":uptime}}),
            &json!({"current":current,"pop_ping_drop_rate":[0.,1.,0.]}),
            3,
        )
        .unwrap()
    }
    #[test]
    fn snapshots_replace_and_repeats_do_not_reset_age() {
        let now = Instant::now();
        let mut s = State::default();
        s.accept(snapshot(5, 10), now);
        s.accept(snapshot(5, 11), now + Duration::from_secs(10));
        assert_eq!(s.advanced_at, Some(now));
        assert_eq!(s.snapshot.as_ref().unwrap().loss_percent.len(), 3);
        assert!(s.notice.as_ref().unwrap().contains("unchanged"));
        s.accept(snapshot(6, 12), now + Duration::from_secs(11));
        assert_eq!(s.advanced_at, Some(now + Duration::from_secs(11)));
        assert!(s.notice.is_none());
    }
    #[test]
    fn failures_keep_old_data_and_recovery_clears_error() {
        let mut s = State::default();
        let now = Instant::now();
        s.fail(anyhow::anyhow!("offline"));
        assert!(s.snapshot.is_none());
        s.accept(snapshot(9, 15), now);
        s.fail(anyhow::anyhow!("offline"));
        assert_eq!(s.snapshot.as_ref().unwrap().current, 9);
        assert_eq!(s.advanced_at, Some(now));
        s.accept(snapshot(1, 1), now + Duration::from_secs(5));
        assert!(s.error.is_none());
        assert!(s.notice.as_ref().unwrap().contains("reset"));
        assert_eq!(s.snapshot.as_ref().unwrap().summary.sample_count, 1);
    }
    #[test]
    fn uptime_reset_detected_even_with_same_counter() {
        let mut s = State::default();
        let now = Instant::now();
        s.accept(snapshot(3, 20), now);
        s.accept(snapshot(3, 4), now + Duration::from_secs(5));
        assert!(s.notice.unwrap().contains("reset"));
        assert_eq!(s.advanced_at, Some(now + Duration::from_secs(5)));
    }
    #[test]
    fn quit_keys_not_other_keys() {
        use crossterm::event::KeyEvent;
        for (code, modifiers) in [
            (KeyCode::Char('q'), KeyModifiers::NONE),
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert!(exit_key(&Event::Key(KeyEvent::new(code, modifiers))));
        }
        assert!(!exit_key(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        ))));
        assert!(!exit_key(&Event::Resize(80, 24)));
    }
}
