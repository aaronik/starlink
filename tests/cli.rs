//! End-to-end CLI tests.  The tiny HTTP server speaks only enough gRPC-Web for
//! the executable; it is deliberately local, bounded, and contains no device data.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use prost::Message;
use starlink::proto;

const BIN: &str = env!("CARGO_BIN_EXE_starlink");
const HISTORY_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/history.json");

fn varint(out: &mut Vec<u8>, mut n: u64) {
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn framed(payload: &[u8], trailer: bool) -> Vec<u8> {
    let mut v = vec![if trailer { 0x80 } else { 0 }];
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}
fn response(tag: u32, inner: Vec<u8>) -> Vec<u8> {
    let mut outer = Vec::new();
    varint(&mut outer, (tag as u64) << 3 | 2);
    varint(&mut outer, inner.len() as u64);
    outer.extend(inner);
    let mut body = framed(&outer, false);
    body.extend(framed(b"grpc-status: 0\r\n", true));
    body
}
fn read_request(stream: &mut TcpStream) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut all = Vec::new();
    let mut buf = [0; 1024];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
        if let Some(headers_end) = all.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&all[..headers_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then_some(value.trim())
                    })
                })
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let request_len = headers_end + 4 + content_length;
            while all.len() < request_len {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0, "request ended before Content-Length bytes arrived");
                all.extend_from_slice(&buf[..n]);
            }
            break;
        }
    }
}
/// Serves exactly these responses, then exits even if a child never connects.
fn server(bodies: Vec<Vec<u8>>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        for body in bodies {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("mock accept: {e}"),
                }
            };
            read_request(&mut stream);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/grpc-web+proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    (endpoint, handle)
}
fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}
fn history() -> Vec<u8> {
    response(
        2006,
        proto::History {
            current: 7,
            pop_ping_drop_rate: vec![1., 0.25, 0., 1., 1.],
            pop_ping_latency_ms: vec![0., 21., 10., 0., 0.],
            downlink_throughput_bps: vec![0., 2e6, 1e6, 0., 0.],
            uplink_throughput_bps: vec![0., 3e5, 1e5, 0., 0.],
            outages: vec![
                proto::Outage {
                    cause: 2,
                    start_timestamp_ns: 200,
                    duration_ns: 1_500_000_000,
                    did_switch: true,
                },
                proto::Outage {
                    cause: 1,
                    start_timestamp_ns: 100,
                    duration_ns: 0,
                    did_switch: false,
                },
            ],
            power_in: vec![50., 51., 52., 53., 54.],
        }
        .encode_to_vec(),
    )
}
fn status() -> Vec<u8> {
    response(
        2004,
        proto::DishStatus {
            device_state: Some(proto::DeviceState { uptime_s: 42 }),
            pop_ping_drop_rate: Some(0.25),
            pop_ping_latency_ms: Some(12.),
            ..Default::default()
        }
        .encode_to_vec(),
    )
}

#[test]
fn help_invalid_values_and_mutations_are_rejected() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Read-only"));
    for args in [
        &["stats", "--window", "0"][..],
        &["watch", "--interval", "0.5"][..],
        &["interruptions", "--min-duration", "NaN"][..],
        &["reboot"][..],
        &["stow"][..],
        &["speed-test"][..],
    ] {
        let out = run(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn analyze_fixture_has_ring_buffer_window_and_errors() {
    let out = run(&["--json", "analyze", HISTORY_FIXTURE, "--window", "3"]);
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["sample_count"], 3);
    assert_eq!(json["available_count"], 5);
    assert_eq!(json["full_loss_sample_seconds"], 2);
    assert_eq!(json["outages"][0]["duration_seconds"], 0.0);
    let human = run(&["analyze", HISTORY_FIXTURE]);
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("5 / 5"));
    for file in ["/definitely/missing-starlink-history.json", "Cargo.toml"] {
        let out = run(&["analyze", file]);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn status_stats_interruptions_and_history_use_real_transport() {
    let (endpoint, join) = server(vec![status(), history(), history(), history()]);
    let out = run(&["--endpoint", &endpoint, "status"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Uptime: 42"));
    let out = run(&["--endpoint", &endpoint, "stats", "--window", "3"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("3 / 5"));
    let out = run(&[
        "--json",
        "--endpoint",
        &endpoint,
        "interruptions",
        "--min-duration",
        "1.5",
    ]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outages"].as_array().unwrap().len(), 2); // zero duration is retained as ambiguous/ongoing
    let out = run(&["--json", "--endpoint", &endpoint, "history"]);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["current"],
        7
    );
    join.join().unwrap();
}

#[test]
fn info_obstructions_and_router_endpoints_are_explicit() {
    let info = response(
        1004,
        proto::GetDeviceInfo {
            device_info: Some(proto::DeviceInfo {
                hardware_version: "synthetic".into(),
                ..Default::default()
            }),
        }
        .encode_to_vec(),
    );
    let bad_grid = response(
        2008,
        proto::ObstructionMap {
            num_rows: 2,
            num_cols: 2,
            snr: vec![1.],
            ..Default::default()
        }
        .encode_to_vec(),
    );
    let (endpoint, join) = server(vec![info, bad_grid]);
    let out = run(&["--endpoint", &endpoint, "info"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("synthetic"));
    let out = run(&["--endpoint", &endpoint, "obstructions"]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    join.join().unwrap();
    let router = response(
        3004,
        proto::RouterStatus {
            ping_latency_ms: Some(9.),
            ..Default::default()
        }
        .encode_to_vec(),
    );
    let clients = response(
        3002,
        proto::Clients {
            clients: vec![proto::ClientEntry {
                name: "test".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
        .encode_to_vec(),
    );
    let (router_endpoint, join) = server(vec![router, clients]);
    for command in [
        vec!["router", "--router-endpoint", &router_endpoint],
        vec!["clients", "--router-endpoint", &router_endpoint],
    ] {
        let out = run(&command);
        assert!(out.status.success());
    }
    join.join().unwrap();
}

#[test]
fn watch_jsonl_success_and_failure_exit_are_finite() {
    let (endpoint, join) = server(vec![status(), history(), status(), history()]);
    let out = run(&[
        "--json",
        "--endpoint",
        &endpoint,
        "watch",
        "--count",
        "2",
        "--interval",
        "1",
    ]);
    assert!(out.status.success());
    let lines: Vec<_> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|v| v["ok"] == true));
    join.join().unwrap();
    // First connection is a valid HTTP response with bad gRPC framing; second poll recovers.
    let (endpoint, join) = server(vec![Vec::new(), status(), history()]);
    let out = run(&[
        "--json",
        "--endpoint",
        &endpoint,
        "watch",
        "--count",
        "2",
        "--interval",
        "1",
    ]);
    assert!(!out.status.success());
    let lines: Vec<_> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["ok"], false);
    assert_eq!(lines[1]["ok"], true);
    join.join().unwrap();
}

#[test]
fn unavailable_one_shot_has_no_stdout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let out = run(&["--timeout", "0.05", "--endpoint", &endpoint, "status"]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}
