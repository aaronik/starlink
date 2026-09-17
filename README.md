# Starlink CLI

A **read-only**, local Rust CLI for Starlink status, statistics, and network interruptions. No Python, Homebrew formula, account credentials, or protobuf compiler required.

This is an unofficial, firmware-dependent telemetry client, **not full mobile-app parity**. It never reboots/stows hardware, changes settings, or starts speed tests.

## Run

```sh
cargo build --release
./target/release/starlink status
./target/release/starlink stats
./target/release/starlink interruptions --min-duration 1
./target/release/starlink watch --window 60 --interval 2
```

Optional installation: `cargo install --path . --locked`.

| Command | Output |
| --- | --- |
| `status` | Uptime, firmware, latency, traffic, GPS, obstruction stats, known active alerts |
| `stats [--window N]` | Retained loss/latency/p95, download/upload, power summary |
| `interruptions [--min-duration S] [--window N]` | Explicit device events plus separate complete-loss sample runs |
| `history` | Full decoded history JSON in **ring-buffer order** |
| `dashboard [--window N] [--interval S]` | Full-screen scrolling charts; q/Esc/Ctrl-C to exit |
| `watch [--count N] [--interval S] [--window N]` | Repeated status/history snapshots; Ctrl-C to stop |
| `obstructions` | Bounded ASCII SNR grid; not a projected sky map |
| `info` | Device identifiers and hardware/firmware data |
| `analyze FILE [--window N]` | Offline analysis of saved `history` output |
| `router --router-endpoint URL` | Partial router telemetry, if the router exposes it |
| `clients --router-endpoint URL` | Partial Wi-Fi client inventory, if available |

Global flags: `--json`, `--timeout SECONDS` (default 5), `--endpoint URL` (default `http://192.168.100.1:9201`). Use `--help` on any command.

Router commands require an explicit `--router-endpoint`; they do not silently try other addresses. Local router gRPC-Web access was **unavailable on the tested network**. These commands are mock-tested, not confirmed working against that router.

## Live graphs

```sh
cargo run -- dashboard
# Five-minute window, or choose a different retained sample window:
cargo run -- dashboard --window 900 --interval 1
```

Full-screen charts refresh in place: **latency, packet loss, download/upload throughput**, plus **power** on wide terminals. Recent explicit interruption events appear below the charts. Press **q**, **Esc**, or **Ctrl-C** to exit; the previous terminal screen is restored.

Use a Unicode/color terminal, ideally **120×36** or larger. At 80×24 the dashboard stacks three compact charts; power is omitted. Below 60×20 it shows a resize hint but remains responsive. Window defaults to 300 one-second samples and is limited by actual dish retention. Polling waits one second after each completed status/history pair by default.

Graphs replace overlapping history snapshots rather than accumulating duplicates. Dots are one-second samples (Braille rendering); missing metrics remain gaps, not zeroes, and full-loss samples have no latency point. Loss uses a fixed 0–100% scale; other charts auto-scale. Throughput is measured traffic, not a speed test. The x-axis ends at the newest **device sample**, not host wall-clock time. Explicit events are separately retained and not restricted to that chart window.

`TELEMETRY UNAVAILABLE` retains the old charts with their age while retrying. `STALE` means the history has not advanced for more than five seconds (also possible with a deliberately long polling interval). These are telemetry freshness indicators, not proof of an internet outage. Counter/uptime resets replace history and show a notice. No persistent history database is created.

The dashboard requires interactive stdin/stdout and rejects `--json`. For pipes/export keep using `watch --json`.

## Export and offline replay

```sh
# Keep live captures private and outside the repository.
umask 077
cargo run -- history > /tmp/my-starlink-history.json
cargo run -- analyze /tmp/my-starlink-history.json --window 300
cargo run -- --json stats --window 60
cargo run -- --json watch --count 10 > /tmp/my-starlink-watch.jsonl
```

Most `--json` commands emit one JSON object; `history` always emits JSON. `watch --json` emits **JSON Lines**, one object per attempted poll, with a host observation timestamp and `ok`. Failed polls contain an error instead of invented zero-loss data. Watch snapshots overlap; do not sum their counters as independent intervals. Watch is not a deduplicating event database.

Exit codes: **0** success/Ctrl-C/closed output pipe; **1** runtime failure (including any failed poll in a finite watch); **2** invalid arguments. Diagnostics go to stderr. JSON uses numeric nanosecond timestamps: consumers using JavaScript must account for integers beyond `Number.MAX_SAFE_INTEGER`.

## Interpret results correctly

- Sample retention varies. The tested dish returned **900 one-second samples**, while its explicit outage list covered a different period. Neither is a permanent outage log.
- `--window` selects the newest samples, **not** explicit outage events. `--min-duration` filters completed explicit events only; zero-duration events remain visible as possibly ongoing.
- Complete-loss runs use samples with 100% loss. Fractional/sub-second interruptions may appear in explicit events without any full-loss samples. Edge runs can be truncated by the retained/window boundary.
- Latency mean/p95 excludes full-loss samples and unavailable/invalid optional readings. p95 uses nearest rank. Throughput is observed traffic, not maximum connection speed.
- Explicit event start times remain raw device nanoseconds: their wall-clock basis has not been established. Sample runs use relative offsets, never invented dates.
- Unknown causes remain `unknown(N)`. The tested firmware already returned a cause absent from the served diagnostic schema.
- `n/a` means a field was not supplied/usable. Proto3 can omit a scalar that equals its default (zero); absence is not proof a metric is unsupported. Known-field alerts and other protobuf fields are only a subset; unknown fields are skipped.

## Connection and privacy

Connect to your Starlink LAN and allow local network access if macOS prompts. This client uses **binary gRPC-Web port 9201**, not the HTTP landing page or native gRPC port 9200. Third-party routers/bypass mode may need a route to the dish. A successful landing page does not guarantee API access. No proxy environment variables or HTTP redirects are used, to avoid routing local telemetry elsewhere.

No account or cloud login is implemented. Device info, clients, maps, and captures can contain identifiers and sensitive usage/installation information. Do not publish live JSON or commit it as a fixture. The CLI never writes captures automatically.

## Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

All automated tests are synthetic and hardware-independent. See [MANUAL_TESTING.md](MANUAL_TESTING.md), [AGENTS.md](AGENTS.md), [TODO.md](TODO.md), and [protocol notes](docs/PROTOCOL.md).
