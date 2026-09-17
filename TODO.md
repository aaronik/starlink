# Roadmap

## Completed: first usable read-only CLI

- [x] Allowlisted binary gRPC-Web transport with deadlines, no proxies/redirects, bounded response body, HTTP/content-type/frame/trailer/application validation.
- [x] Audited minimal protobuf schema from the locally served diagnostics bundle; no build-time downloads or protobuf compiler.
- [x] `status`, `info`: dish telemetry, selected alerts, GPS and obstruction statistics.
- [x] `stats`, `history`, `interruptions`: ring-buffer analysis, optional sample window, mean/p95 latency, download/upload, power, explicit cause/duration filtering, separate sample-loss runs.
- [x] `watch`: human/JSONL snapshots, bounded count or Ctrl-C, retry after failed polls, explicit errors, no false zero-loss substitution.
- [x] JSON exports via stdout and offline `analyze` replay.
- [x] `obstructions`: validated grid and bounded relative-SNR ASCII visualization; full JSON export.
- [x] `router` / `clients`: explicit endpoint, verified subset, synthetic tests. **Live router endpoint unavailable on this network; not hardware-verified.**
- [x] Unit, mock transport, and actual-binary CLI tests with synthetic fixtures; no hardware required.
- [x] Live dish checks and failure/termination testing recorded in MANUAL_TESTING.md.
- [x] README, AGENTS.md, protocol notes, manual testing guide.

## Next priorities

- [ ] Compare matched windows/filters with the actual mobile app; document aggregation differences.
- [ ] Establish explicit outage timestamp epoch/basis across firmware before displaying wall-clock event dates; do not infer from host time.
- [ ] Investigate newer outage cause values (observed 13) with authoritative schema evidence; keep unknown numbers intact meanwhile.
- [ ] Persistent, deduplicated local sample/event storage with reboot detection, gap tracking, retention limits, and private files. Current watch snapshots overlap and are not such a store.
- [ ] Longer monitoring soak and spontaneous disconnect/reconnect testing; do not induce hardware outages or mutations.
- [x] Full-screen `dashboard`: scrolling latency/loss/traffic/power charts, recent events, stale/error handling, adaptive layout, clean terminal restoration. Hardware-tested and Unix PTY-tested.
- [ ] Further timeline/TUI enhancements (interactive zoom, event scrolling), CSV/Prometheus exports, and read-only threshold notifications.
- [ ] Validate router transport/routing on supported hardware; extend mesh topology/client/radio schemas only with verified tags.
- [ ] Expand alert/configuration-read schema and model omitted defaults versus truly unavailable fields explicitly.
- [ ] Correct sky projection/orientation visualization; current grid is only relative SNR.
- [ ] Additional firmware/hardware fixtures (synthetic or explicitly sanitized) and compatibility CI.

## Scope boundary

Aim for useful **read-only local mobile-app telemetry**, not all app features. Account login, billing, support, cloud history, and remote management are not implemented. Reboot, stow, settings/configuration writes, and speed-test initiation are deliberately excluded, not future CLI mutations.

Read-only does not mean zero traffic: each poll sends diagnostic requests. Polls are sequential and the interval is at least one second after completion. No arbitrary RPC interface or automatic endpoint discovery is exposed.
