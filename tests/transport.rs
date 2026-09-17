use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use starlink::{
    client::{Client, Query},
    proto,
};

fn varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
fn frame(payload: &[u8], trailer: bool) -> Vec<u8> {
    let mut out = vec![if trailer { 0x80 } else { 0 }];
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}
fn response_body(tag: u32) -> Vec<u8> {
    let mut outer = Vec::new();
    varint(&mut outer, tag << 3 | 2);
    outer.push(0); // an empty, valid inner response
    let mut body = frame(&outer, false);
    body.extend(frame(b"grpc-status: 0\r\n", true));
    body
}
fn serve(
    status: &str,
    content_type: &str,
    body: Vec<u8>,
    delay: Duration,
) -> (String, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let content_type = content_type.to_owned();
    let handle = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "mock accept timed out"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("mock accept: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0, "client closed before complete request");
            request.extend_from_slice(&buffer[..read]);
            if let Some(head_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..head_end + 4]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length: ")
                            .or_else(|| line.strip_prefix("Content-Length: "))
                    })
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                if request.len() >= head_end + 4 + length {
                    break;
                }
            }
        }
        thread::sleep(delay);
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let result = stream
            .write_all(header.as_bytes())
            .and_then(|()| stream.write_all(&body));
        if delay.is_zero() {
            result.unwrap();
        }
        request
    });
    (format!("http://{address}/prefix"), handle)
}

#[tokio::test]
async fn every_allowlisted_query_sends_exact_frame_and_decodes() {
    for (query, tag, request) in [
        (Query::Status, 2004, vec![0xe2, 0x3e, 0]),
        (Query::History, 2006, vec![0xfa, 0x3e, 0]),
        (Query::Obstructions, 2008, vec![0xc2, 0x7d, 0]),
        (Query::DeviceInfo, 1004, vec![0x82, 0x3f, 0]),
        (Query::RouterStatus, 3004, vec![0xe2, 0x3e, 0]),
        (Query::Clients, 3002, vec![0xd2, 0xbb, 0x01, 0]),
    ] {
        let (endpoint, server) = serve(
            "200 OK",
            "application/grpc-web+proto",
            response_body(tag),
            Duration::ZERO,
        );
        let value = Client::new(&endpoint, Duration::from_secs(1))
            .unwrap()
            .query(query)
            .await
            .unwrap();
        assert!(value.is_object());
        let received = server.join().unwrap();
        let head_end = received.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let header = String::from_utf8_lossy(&received[..head_end]).to_ascii_lowercase();
        assert!(
            header.starts_with(
                "post /prefix/spaceX.api.device.device/handle http/1.1"
                    .to_ascii_lowercase()
                    .as_str()
            )
        );
        assert!(header.contains("content-type: application/grpc-web+proto"));
        assert!(header.contains("x-grpc-web: 1"));
        assert_eq!(&received[head_end..], frame(&request, false));
    }
}

#[tokio::test]
async fn rejects_http_grpc_application_and_payload_errors() {
    let cases = vec![
        ("500 Nope", "application/grpc-web+proto", vec![], "HTTP 500"),
        ("200 OK", "text/plain", response_body(2004), "content type"),
        (
            "200 OK",
            "application/grpc-web+proto",
            frame(b"grpc-status: 7\r\ngrpc-message: denied\r\n", true),
            "status 7",
        ),
        (
            "200 OK",
            "application/grpc-web+proto",
            {
                let mut b = Vec::new();
                b.extend(frame(&[0x12, 2, 8, 1], false));
                b.extend(frame(b"grpc-status: 0\r\n", true));
                b
            },
            "application status 1",
        ),
        (
            "200 OK",
            "application/grpc-web+proto",
            response_body(2006),
            "expected read-only variant",
        ),
        (
            "200 OK",
            "application/grpc-web+proto",
            {
                let mut b = frame(&[0xff], false);
                b.extend(frame(b"grpc-status: 0\r\n", true));
                b
            },
            "invalid Device.Response",
        ),
    ];
    for (status, content_type, body, error) in cases {
        let (endpoint, server) = serve(status, content_type, body, Duration::ZERO);
        let result = Client::new(&endpoint, Duration::from_secs(1))
            .unwrap()
            .query(Query::Status)
            .await;
        assert!(result.unwrap_err().to_string().contains(error));
        server.join().unwrap();
    }
}

#[tokio::test]
async fn timeout_and_redirect_are_not_followed() {
    let (endpoint, server) = serve(
        "302 Found",
        "application/grpc-web+proto",
        vec![],
        Duration::ZERO,
    );
    assert!(
        Client::new(&endpoint, Duration::from_secs(1))
            .unwrap()
            .query(Query::Status)
            .await
            .unwrap_err()
            .to_string()
            .contains("HTTP 302")
    );
    server.join().unwrap();

    let (endpoint, server) = serve(
        "200 OK",
        "application/grpc-web+proto",
        response_body(2004),
        Duration::from_millis(100),
    );
    assert!(
        Client::new(&endpoint, Duration::from_millis(10))
            .unwrap()
            .query(Query::Status)
            .await
            .is_err()
    );
    server.join().unwrap();
}

#[test]
fn audited_client_entry_wire_fields_are_not_guesses() {
    use prost::Message;
    let client = proto::ClientEntry {
        name: "n".into(),
        mac_address: "m".into(),
        ip_address: "i".into(),
        signal_strength: Some(-55.0),
        client_id: Some(9),
        active: Some(true),
    };
    let encoded = client.encode_to_vec();
    assert_eq!(
        proto::ClientEntry::decode(encoded.as_slice()).unwrap(),
        client
    );
}
