//! Unix PTY coverage for the interactive dashboard.  These tests use only a
//! loopback synthetic gRPC-Web peer; they never contact a dish.
#![cfg(unix)]

use std::{
    ffi::c_void,
    fs::File,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use prost::Message;
use starlink::proto;

const BIN: &str = env!("CARGO_BIN_EXE_starlink");
const ALT_ON: &[u8] = b"\x1b[?1049h";
const ALT_OFF: &[u8] = b"\x1b[?1049l";

fn framed(payload: &[u8], trailer: bool) -> Vec<u8> {
    let mut out = vec![if trailer { 0x80 } else { 0 }, 0, 0, 0, payload.len() as u8];
    out.extend_from_slice(payload);
    out
}
fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push(value as u8 | 128);
        value >>= 7;
    }
    out.push(value as u8);
}
fn response(tag: u32, message: impl Message) -> Vec<u8> {
    let payload = message.encode_to_vec();
    let mut outer = Vec::new();
    // All response messages used here have a length-delimited outer field.
    varint(&mut outer, ((tag << 3) | 2) as u64);
    varint(&mut outer, payload.len() as u64);
    outer.extend(payload);
    let mut body = framed(&outer, false);
    body.extend(framed(b"grpc-status: 0\r\n", true));
    body
}
fn status() -> Vec<u8> {
    response(
        2004,
        proto::DishStatus {
            device_state: Some(proto::DeviceState { uptime_s: 17 }),
            pop_ping_drop_rate: Some(0.0),
            pop_ping_latency_ms: Some(21.0),
            ..Default::default()
        },
    )
}
fn history(current: u64) -> Vec<u8> {
    response(
        2006,
        proto::History {
            current,
            pop_ping_drop_rate: vec![0., 0.1, 0.],
            pop_ping_latency_ms: vec![20., 30., 22.],
            downlink_throughput_bps: vec![1_000_000., 2_000_000., 3_000_000.],
            uplink_throughput_bps: vec![100_000., 200_000., 300_000.],
            power_in: vec![50., 51., 52.],
            ..Default::default()
        },
    )
}
fn read_request(stream: &mut TcpStream) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut all = Vec::new();
    let mut buf = [0; 1024];
    loop {
        let n = stream
            .read(&mut buf)
            .expect("complete mock request before timeout");
        assert!(n > 0, "mock request closed before headers");
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
/// Each accepted request receives the next action. `None` deliberately holds
/// the socket open, exercising cancellation of an in-flight request.
fn mock(actions: Vec<Option<Vec<u8>>>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let join = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        for action in actions {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("mock accept: {e}"),
                }
            };
            if let Some(body) = action {
                read_request(&mut stream);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/grpc-web+proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(&body).unwrap();
            } else {
                thread::sleep(Duration::from_secs(2));
            }
        }
    });
    (endpoint, join)
}

struct Pty {
    master: File,
    child: Child,
    seen: Vec<u8>,
    parser: vt100::Parser,
}
impl Pty {
    fn spawn(endpoint: &str) -> Self {
        unsafe {
            let mut master = 0;
            let mut slave = 0;
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                0,
                "openpty"
            );
            let master = File::from_raw_fd(master);
            let slave = File::from_raw_fd(slave);
            // Do not leak either openpty endpoint to unrelated test subprocesses.
            for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                assert_ne!(flags, -1);
                assert_ne!(libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC), -1);
            }
            let ws = libc::winsize {
                ws_row: 40,
                ws_col: 120,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            assert_eq!(
                libc::ioctl(
                    master.as_raw_fd(),
                    libc::TIOCSWINSZ,
                    &ws as *const _ as *const c_void
                ),
                0
            );
            // Clones are intentional child stdio; the originals remain close-on-exec.
            // dup-created descriptors are the intentional stdio inherited by the child.
            let input = slave.try_clone().unwrap();
            let output = slave.try_clone().unwrap();
            let error = slave.try_clone().unwrap();
            let mut command = Command::new(BIN);
            command
                .args([
                    "--endpoint",
                    endpoint,
                    "--timeout",
                    "5",
                    "dashboard",
                    "--interval",
                    "1",
                    "--window",
                    "3",
                ])
                .stdin(Stdio::from(input))
                .stdout(Stdio::from(output))
                .stderr(Stdio::from(error));
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
            let child = command.spawn().unwrap();
            let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
            assert_ne!(flags, -1);
            assert_ne!(
                libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK),
                -1
            );
            Self {
                master,
                child,
                seen: Vec::new(),
                parser: vt100::Parser::new(40, 120, 0),
            }
        }
    }
    fn write(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }
    fn screen(&self) -> String {
        self.parser.screen().contents()
    }
    fn read_available(&mut self) {
        let mut buf = [0; 8192];
        loop {
            match self.master.read(&mut buf) {
                Ok(n) if n > 0 => {
                    self.seen.extend_from_slice(&buf[..n]);
                    self.parser.process(&buf[..n]);
                }
                Ok(_) => break,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                Err(e) => panic!("pty read: {e}"),
            }
        }
    }
    fn output_until(&mut self, needle: &str, timeout: Duration) -> Vec<u8> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.read_available();
            if self.screen().contains(needle) {
                return self.seen.clone();
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("did not see {needle:?}; screen was {:?}", self.screen());
    }
    fn raw_output_until(&mut self, needle: &[u8], timeout: Duration) -> Vec<u8> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.read_available();
            if self.seen.windows(needle.len()).any(|w| w == needle) {
                return self.seen.clone();
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("did not see raw {:?}", String::from_utf8_lossy(needle));
    }
    fn exit_with(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.write(bytes);
        let started = Instant::now();
        let status = loop {
            self.read_available();
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "dashboard did not cancel stalled request"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "{status}");
        self.raw_output_until(ALT_OFF, Duration::from_secs(1))
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[test]
fn dashboard_draws_resizes_and_restores_terminal_on_quit() {
    let (endpoint, server) = mock(vec![Some(status()), Some(history(3))]);
    let mut pty = Pty::spawn(&endpoint);
    let initial = pty.output_until("Packet loss", Duration::from_secs(2));
    assert!(initial.windows(ALT_ON.len()).any(|w| w == ALT_ON));
    assert!(pty.screen().contains("Latency (ms)"));
    unsafe {
        let ws = libc::winsize {
            ws_row: 30,
            ws_col: 110,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            libc::ioctl(
                pty.master.as_raw_fd(),
                libc::TIOCSWINSZ,
                &ws as *const _ as *const c_void
            ),
            0
        );
        assert_eq!(libc::kill(pty.child.id() as i32, libc::SIGWINCH), 0);
    }
    thread::sleep(Duration::from_millis(100));
    assert!(pty.child.try_wait().unwrap().is_none());
    let final_output = pty.exit_with(b"q");
    assert!(final_output.windows(ALT_OFF.len()).any(|w| w == ALT_OFF));
    unsafe {
        let mut term = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(pty.master.as_raw_fd(), &mut term), 0);
        assert_ne!(term.c_lflag & libc::ICANON, 0);
        assert_ne!(term.c_lflag & libc::ECHO, 0);
    }
    server.join().unwrap();
}

#[test]
fn dashboard_keeps_stale_error_until_a_later_success() {
    let (endpoint, server) = mock(vec![
        Some(status()),
        Some(history(3)),
        Some(status()),
        Some(vec![0]), // malformed gRPC-Web history body: a poll failure, not zero loss
        Some(status()),
        Some(history(4)),
    ]);
    let mut pty = Pty::spawn(&endpoint);
    pty.output_until("Packet loss", Duration::from_secs(2));
    let unavailable = pty.output_until("TELEMETRY UNAVAILABLE", Duration::from_secs(2));
    assert!(pty.screen().contains("Packet loss"));
    // The error screen has replaced the earlier live screen; require a later redraw.
    assert!(!pty.screen().contains("LIVE: chart last advanced"));
    let error_bytes = unavailable.len();
    let recovered = pty.output_until("LIVE: chart last advanced", Duration::from_secs(2));
    assert!(recovered.len() > error_bytes);
    assert!(pty.screen().contains("Packet loss"));
    pty.exit_with(b"q");
    server.join().unwrap();
}

#[test]
fn dashboard_input_and_sigterm_cancel_stalled_requests() {
    for key in [b"q".as_slice(), b"\x03".as_slice()] {
        let (endpoint, server) = mock(vec![None]);
        let mut pty = Pty::spawn(&endpoint);
        pty.output_until("WAITING", Duration::from_secs(1));
        thread::sleep(Duration::from_millis(250));
        pty.exit_with(key);
        server.join().unwrap();
    }
    let (endpoint, server) = mock(vec![None]);
    let mut pty = Pty::spawn(&endpoint);
    pty.output_until("WAITING", Duration::from_secs(1));
    thread::sleep(Duration::from_millis(250));
    unsafe {
        assert_eq!(libc::kill(pty.child.id() as i32, libc::SIGTERM), 0);
    }
    let started = Instant::now();
    while pty.child.try_wait().unwrap().is_none() {
        pty.read_available();
        assert!(started.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_millis(10));
    }
    server.join().unwrap();
}
