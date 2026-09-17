# Local diagnostic protocol notes

## Boundary and evidence

This document describes the narrow, read-only protobuf subset implemented in this repository. It is **not** a complete or stable public Starlink API specification. The field/tag observations used here were checked against the JavaScript served by the local Starlink web UI; they are not inferred from cloud/mobile-app traffic. Firmware can add, remove, or reinterpret fields.

The client deliberately has no generic RPC facility and permits only its read-query allowlist. It does not send reboot, stow, settings/configuration, speed-test, or other mutation requests.

## Transport

The verified local service is binary gRPC-Web over HTTP(S), on port **9201** when the endpoint is selected as, for example, `http://<device>:9201/`:

```text
POST /SpaceX.API.Device.Device/Handle
Content-Type: application/grpc-web+proto
Accept: application/grpc-web+proto
x-grpc-web: 1
```

Requests and responses are gRPC-Web binary frames: a one-byte flags field, a four-byte big-endian length, then the payload. A unary response has one message frame followed by a trailer frame with `grpc-status: 0`. The client rejects redirects, non-success HTTP responses, malformed/multiple response frames, missing/failing trailers, and oversized responses.

The endpoint is explicit. In particular, router reads are made against the endpoint chosen by the user; this tool does not silently probe or guess a dish/router route.

## Verified implemented request and response variants

All requests below have an empty embedded message. Tags are Device.Request / Device.Response field numbers.

| Read query | Request field/tag | Response field/tag | Decoded inner message |
| --- | ---: | ---: | --- |
| Dish status | `get_status` / 1004 | `dish_get_status` / 2004 | `DishStatus` |
| History | `get_history` / 1007 | `dish_get_history` / 2006 | `History` |
| Device information | `get_device_info` / 1008 | `get_device_info` / 1004 | `GetDeviceInfo` |
| Obstruction map | `dish_get_obstruction_map` / 2008 | `dish_get_obstruction_map` / 2008 | `ObstructionMap` |
| Router status | `get_status` / 1004 | `wifi_get_status` / 3004 | `RouterStatus` |
| Wi-Fi clients | `wifi_get_clients` / 3002 | `wifi_get_clients` / 3002 | `Clients` |

The history request is specifically **`get_history` (1007)**. It is not a `dish_get_history` request. `dish_get_history` is the **response** variant (2006).

After decoding the outer response, the client returns JSON for only the inner message, with `snake_case` field names. Protobuf integer fields serialize as JSON numbers.

## Subset limits

The Rust structs intentionally decode only fields needed by the initial CLI: selected device data, status/alerts, obstruction statistics/map, retained history arrays/outages, limited router status, and a small client-entry shape. Prost skips unknown protobuf fields, so newer firmware data can be absent from output without an error. Conversely, a present field should not be assumed universal across hardware generations or firmware.

The decoded alerts are a partial known-field list, not a promise to report every alert the app can display. Router/client/mesh topology is incomplete and varies by router generation. No account, cloud, authentication, billing, support, or remote-management protocol is implemented.

## History and interruption interpretation

`History.current` and its metric arrays are treated as a circular sequence of one-second samples. Analysis uses `min(current, array_capacity)` samples and rotates a full buffer at `current % capacity`; it reports offsets in the selected chronological window. This is a practical interpretation of the locally served schema, not a guarantee that every device retains 12 hours (or any fixed duration) of history. Retention depends on capacity, firmware, uptime, and available fields.

Two interruption signals must remain separate:

- A full-loss sample run is a contiguous run of retained `pop_ping_drop_rate >= 1.0` samples. It is sample-derived and limited to the selected retained window.
- An explicit outage is an item in `outages`, with a cause enum, duration, switch flag, and `start_timestamp_ns`. It is device-reported and need not have a one-to-one correspondence with full-loss samples.

`start_timestamp_ns` is a signed int64 preserved as returned; duration is an unsigned uint64. This project does **not** claim that the start time has been validated as a wall-clock timestamp, and it does not fabricate wall-clock times from sample offsets. The analysis `ongoing` flag only means zero duration (possibly ongoing), not a proven live outage. Sample windows never filter the independently retained explicit event list.

The checked-in Rust schema is self-contained: `/tmp/starlink-script.js` was used during development but is not needed to build/run. Refresh observations from `http://192.168.100.1/static/js/script.js.gz` when auditing a new firmware; do not execute downloaded code or guess field types. Protobuf optional scalars preserve wire presence, but an omitted proto3 zero cannot be distinguished from an unavailable field without additional device semantics. JSON `null` is deliberately not relabeled as measured zero.

## Privacy and safe testing

Local status, client lists, device information, obstruction data, and histories may reveal device identifiers, LAN addresses/MAC addresses, usage patterns, and potentially location-related installation information. Do not commit raw live responses, device IDs, addresses, coordinates, or obstruction captures. Tests use synthetic fixtures and do not require hardware. Live checks, if performed manually, must use only the read allowlist and should keep output private.
