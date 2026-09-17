# Agent guide

## Goal and nonnegotiable safety boundary
Build a local, read-only Rust CLI for Starlink telemetry, particularly interruptions.
Never send reboot, stow, configuration changes, speed-test initiation, or other mutation RPCs.
Do not promise complete mobile-app parity: account/cloud features and mutations are outside scope.
Live manual checks are allowed for read allowlist only. Do not create real outages for tests.
Never commit device IDs, location information, MACs, raw live responses, or private captures.

## Architecture
- `src/proto.rs`: audited minimal Prost schema; `Query` enum is the sole request allowlist.
  Request = get_status (1004), get_history (1007), get_device_info (1008),
  dish_get_obstruction_map (2008), wifi_get_clients (3002). No arbitrary request encoding API.
  `get_history` is the request; `dish_get_history` is its response, not the request.
- `src/client.rs`: `Client::new(&str, Duration) -> Result<Client>` and async
  `query(Query) -> Result<serde_json::Value>`. Inner snake_case JSON; integer numbers.
  Binary gRPC-Web at port 9201. Bounded 16 MiB body, timeout, no redirects/proxies,
  strict HTTP/content-type/frame/trailer/application validation. No native 9200 transport yet.
- `src/history.rs`: pure `analyze(&Value, Option<usize>) -> Result<Summary>`.
  Circular one-second samples; current is next-write counter, rotate at current % capacity,
  valid min(current, capacity). Optional absent/null/negative metrics excluded from averages;
  invalid written loss rejected; unwritten sentinel padding ignored. Explicit outages remain
  independent of sample window. Signed start timestamps, unsigned duration nanoseconds.
- `src/render.rs`: plain safe terminal formatting and bounded area-averaged SNR grid.
  Escape device control/bidi characters. Never call the grid a projected sky obstruction map.
- `src/main.rs`: clap commands, JSON, offline replay, sequential polling with Ctrl-C.
  See README for command contract. Finite watch exits 1 if any poll failed; failed polls are
  unavailable telemetry, NOT automatically Starlink outages. JSONL snapshots overlap.
- `tests/transport.rs`: synthetic HTTP/gRPC-Web wire tests.
- `tests/cli.rs`: actual-binary mock transport and offline/argument/watch tests.
- `tests/fixtures/history.json`: synthetic data ONLY.

## Commands and validation

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
./target/release/starlink interruptions --min-duration 1
./target/release/starlink watch --count 3
```

Tests never require a live dish; mock servers must have bounded waits. Keep Cargo.lock for the CLI.
CI workflow runs these test/lint gates on macOS and Linux (configured, remote CI not yet observed).
See MANUAL_TESTING.md for repeatable live/failure/exit-code checks and actual results.
Update TODO.md, protocol notes, manual results, and this file whenever behavior changes.

## Current checkpoint — 2026-09-16

First usable CLI implemented in `/Users/aaron/projects/starlink`: status, stats, interruptions,
history export, watch, info, obstruction grid, offline analyze, partial router/client commands.
README and TODO.md describe precise scope, usage, and next steps. No git commits created.

Validation: **37 automated tests passed** (25 library, 2 parser, 6 executable CLI, 4 transport),
strict clippy clean, formatting clean, debug/release builds successful. Live dish commands,
three-poll human/JSONL watch, private export/offline independent math check, refusal errors,
invalid args/mutation rejection, Ctrl-C, and closed pipe manually verified. No persistent monitor
was left running and private test capture was deleted.

Observed firmware returned 900 history samples and a separate ~120 event list. Event timestamp
basis is NOT established; keep raw nanos and relative sample offsets. Unknown cause 13 must
remain unknown until independently verified. Zero event duration means *possibly* ongoing;
`ongoing` boolean is only that compatibility indicator. Proto3 omitted scalar defaults may
appear n/a; do not assert absence proves unsupported telemetry. Known schema is partial.

Router API at 192.168.1.1:9201 failed locally with No route to host. Router/client commands are
synthetic-tested but NOT hardware-verified. Mobile-app comparison, long soak, durable deduplicated
storage/reboot-gap tracking, wall-clock event presentation, mesh coverage remain TODO.

## Schema maintenance and collaboration
Schema facts were audited against the served diagnostics JS; no downloaded bundle is required at
build/runtime. See docs/PROTOCOL.md. Never guess field tags/types (initial guessed Wi-Fi client
fields were corrected during audit). Add synthetic wire coverage when changing the subset.
Parallel agents should own disjoint files; coordinating agent owns dependencies/integration and
must run the actual integrated tests (isolated agent tests alone are insufficient).

## Dashboard checkpoint — 2026-09-16

`dashboard --window 300 --interval 1` now provides full-screen live graphs via Ratatui 0.29 /
Crossterm 0.28. `src/dashboard/view.rs` owns pure snapshot rotation and adaptive rendering;
`src/dashboard/mod.rs` owns cancellable sequential status/history reads, bounded zero-timeout
keyboard polling every 50 ms and redraw every 200 ms. Runtime uses no arbitrary/mutating RPC.
A terminal guard restores raw/alternate-screen/cursor state on exit/errors, with panic restoration.
q, Esc, Ctrl-C and Unix SIGTERM quit. Interactive stdin/stdout required; no dashboard JSON.

Never append overlapping history arrays: each validated Snapshot replaces the old one. Same
counter does not refresh chart age; decreasing counter/uptime shows reset notice. Failed polls
retain old data and original age with TELEMETRY UNAVAILABLE. STALE threshold is >5 seconds
without advancement (not an internet-health diagnosis, and long intervals can cause it).
Loss chart is fixed 0..100%; other axes auto-scale. Braille scatter points avoid connecting gaps.
Explicit events stay independent of selected samples; show raw start nanos, readable cause and
seconds duration. At >=100 columns power is included; narrower terminals show three graphs;
below 60x20 show a hint. README documents limitations and recommended 120x36 size.

Current validation: **55 tests passed** (38 lib, 2 parser, 6 existing CLI, 2 dashboard CLI,
3 Unix PTY, 4 transport), strict clippy/fmt clean, release built. Live PTY updating/resizing/
q and refused-endpoint Ctrl-C verified; full procedure/results in MANUAL_TESTING.md.
Dev dependencies libc/vt100 are test-only. Unix PTY tests parse actual terminal state and must
drain output while waiting for process exit (otherwise redraw fills the PTY and blocks).
Mock TCP listeners are nonblocking but accepted sockets MUST explicitly return to blocking mode
on macOS; consume full Content-Length before responding to avoid RST/false sequence failures.
Do not weaken these tests to ignore unavailable telemetry. No monitor remains running.
