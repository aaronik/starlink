# Manual testing

## Safety and setup

Only use the documented read-only CLI commands. Never reboot, stow, disconnect the dish, change routing/settings, or start speed tests to simulate failures. Use a refused loopback endpoint or a synthetic server instead. Keep live captures out of git; use private temporary files and remove them after testing. Do not record device IDs, MACs, coordinates, or raw live captures here.

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
BIN=./target/debug/starlink
```

## Live happy paths

On the Starlink LAN:

```sh
$BIN --help
$BIN status
$BIN stats
$BIN stats --window 60
$BIN interruptions
$BIN interruptions --min-duration 1 --window 300
$BIN obstructions
$BIN info
$BIN watch --count 3 --interval 1 --window 60
$BIN --json watch --count 3 --interval 1
```

Judge readable units/alignment, plausible changing latency/traffic, increasing uptime, bounded grid size, no escape sequences from device strings, and clear distinction between complete-loss samples and explicit events. Empty events are not proof of a flawless connection. `n/a` must not be fabricated zero telemetry. `unknown(N)` must preserve unfamiliar causes. The grid is relative SNR, not a claimed classification/projection. Watch should stop at count, avoid overlapping requests, and emit parseable JSONL with increasing **host** observation timestamps.

Compare against the mobile app when available: use matching duration filters/time intervals. Exact agreement is **not** assumed: the app may have different retention/aggregation, and device event timestamp basis is not established here. Agent does not have app access; this comparison remains pending.

## Export/replay and independent sample check

```sh
umask 077
CAPTURE=$(mktemp)
$BIN history > "$CAPTURE"
$BIN analyze "$CAPTURE" --window 60
$BIN --json analyze "$CAPTURE" --window 60
# Inspect locally with Python or jq; never commit the capture.
rm "$CAPTURE"
```

Independent check: for capacity N and current C, take first C entries if C<N; otherwise rotate at C%N. Select newest requested samples, average packet-loss fractions, compare with `mean_packet_loss`. History arrays are in physical ring order, not chronological order. Explicit events are not window-filtered. Optional scalar JSON nulls are preserved.

## Stop and pipe behavior

```sh
$BIN watch --interval 10
# Press Ctrl-C during polling or sleep: exit 0, no stack trace.
$BIN history | head -n 1
# Closed stdout should end cleanly, without broken-pipe panic.
```

## Failure paths (no real outage needed)

```sh
$BIN --endpoint http://127.0.0.1:1 --timeout 0.1 status
# Exit 1, actionable stderr, no success output.
$BIN --json --endpoint http://127.0.0.1:1 --timeout 0.1 watch --count 2 --interval 1
# Two JSONL ok:false error records, then exit 1; never loss=0.
$BIN watch --interval nan
$BIN stats --window 0
$BIN reboot
# Each exits 2 before making any network request.
$BIN analyze /nonexistent/history.json
# Exit 1, file error on stderr.
```

Tests cover HTTP/gRPC/application errors, schema mismatch, bad frames/content type, timeouts, watch recovery, and malformed offline data without hardware. Use `cargo test --test transport` and `cargo test --test cli`.

## Optional router checks

```sh
$BIN --timeout 2 router --router-endpoint http://192.168.1.1:9201
$BIN --timeout 2 clients --router-endpoint http://192.168.1.1:9201
```

Only use an endpoint belonging to your router. A failed API connection does not prove the router is offline; its landing page may still work. Do not silently probe other endpoints or claim router support is verified if only mock-tested.

## Execution record — initial implementation, 2026-09-16

- Automated tests, formatting, and strict clippy run; final counts recorded in AGENTS.md.
- Live status, stats, selected windows, interruption filter, device info, obstruction grid: successful. Output reviewed directly in terminal.
- Served firmware `2026.08.17.cr84795.52288`; 900 retained samples and separate ~120 events (count changed during testing). Unknown cause 13 correctly kept unknown. No live raw data committed.
- Observed nonzero partial loss with zero complete-loss samples; presentation correctly distinguishes these from explicit outage records.
- Grid decoded as 123x123; bounded visualization rendered without flooding the terminal.
- Three-poll human and JSONL watch: successful, finite, updating timestamps/telemetry.
- Live history private temporary export -> offline 60-sample JSON summary: successful; independent Python rotation/loss calculation matched. Temporary file deleted.
- Refused endpoint one-shot and two-poll watch: correct runtime error exits and no fabricated network-health data.
- Invalid interval/window and reboot command: correct usage-error exits.
- SIGINT watch during 10-second sleep: clean exit 0; closed stdout on history: clean exit 0.
- Router and clients at `192.168.1.1:9201`: **unavailable**, returned local connection error (`No route to host`), exit 1. Router commands remain hardware-unverified.
- Mobile-app side-by-side comparison, long-duration soak, reconnect across real spontaneous dish reboot, and other hardware/firmware generations: **not performed**.

## Live graph dashboard — added 2026-09-16

```sh
cargo run -- dashboard
cargo run -- dashboard --window 900 --interval 1
# q / Esc / Ctrl-C quits; use a Unicode terminal, preferably 120x36.
cargo run -- --endpoint http://127.0.0.1:1 --timeout 0.1 dashboard
# Error stays visible; quit works even during requests/retries.
cargo run -- dashboard | cat
cargo run -- dashboard --json
# Both must fail locally, without alternate-screen escape output.
```

Check charts are populated from retained history immediately, scrolling with new samples rather
than accumulating duplicate snapshots. Verify loss is percent, latency ms, traffic Mbps, power W;
optional missing values show n/a/gaps. Relative sample-time x-axis is not a wall-clock event time.
Resize from 120x36 -> 80x24 -> 50x15 -> 120x36; wide mode includes power, narrow mode stacks
three graphs, tiny mode shows resize hint. Quit must restore canonical input, echo, cursor, and
previous screen. For synthetic success -> failed poll -> recovery, run:

```sh
cargo test --test dashboard_pty
cargo test dashboard::
```

Actual checks performed:
- Live status/history rendered through the dashboard at 120x36 and 80x24; graph shapes, units,
  relative axis, and explicit event list visually inspected as rendered terminal-cell output.
- Actual live PTY CLI ran multiple updates, resized wide/narrow/tiny/back, and quit with q.
  Verified alternate-screen exit, ICANON and ECHO restoration. No raw capture saved in repo.
- Final release binary exercised against live dish and refused loopback endpoint in PTYs;
  q and Ctrl-C byte exits successful, canonical mode restored.
- Automated PTY tests check full terminal output via VT100 emulation (not brittle ANSI substring
  assumptions), resize, q/Ctrl-C/SIGTERM cancellation, retained-chart error visibility and recovery.
- 55 tests passed overall; strict Clippy and formatting clean; release built. macOS tested locally;
  Linux CI configured but not observed during this session. No hours-long soak performed.
