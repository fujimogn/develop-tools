//! `HttpTransport` を手元の偽の server で回す。本物の API は呼ばない。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use island_llm::transport::{ApiRequest, FailureKind, HttpTransport, Transport};
use island_secrets::Secret;

/// 1 回だけ接続を受け、request を読んでから `respond` に stream を渡す。
fn server(respond: impl FnOnce(std::net::TcpStream) + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake API");
    let address = listener.local_addr().expect("fake API address");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buffer = [0_u8; 4096];
        let _ = stream.read(&mut buffer);
        respond(stream);
    });
    format!("http://{address}")
}

fn send(
    base_url: String,
    timeout: Duration,
) -> Result<island_llm::transport::ApiResponse, island_llm::transport::TransportError> {
    let key = Secret::new("test-api-key");
    HttpTransport { base_url, timeout }.send(&ApiRequest {
        body: serde_json::json!({}),
        api_key: &key,
    })
}

#[test]
fn a_body_cut_after_the_headers_keeps_the_request_id() {
    let base_url = server(|mut stream| {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nrequest-id: req_cut\r\n\r\n{\"content\"")
            .expect("write partial response");
    });

    let error = send(base_url, Duration::from_secs(5)).expect_err("cut body");

    assert_eq!(error.kind, FailureKind::Failed);
    assert_eq!(error.request_id.as_deref(), Some("req_cut"));
}

#[test]
fn no_response_within_the_timeout_is_a_timeout() {
    let base_url = server(|stream| {
        thread::sleep(Duration::from_secs(3));
        drop(stream);
    });

    let error = send(base_url, Duration::from_millis(200)).expect_err("timeout");

    assert_eq!(error.kind, FailureKind::Timeout);
}

#[test]
fn an_error_status_is_returned_as_a_response() {
    let base_url = server(|mut stream| {
        stream
            .write_all(
                b"HTTP/1.1 529 Overloaded\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            )
            .expect("write response");
    });

    let response = send(base_url, Duration::from_secs(5)).expect("status is not an error");

    assert_eq!(response.status, 529);
    assert_eq!(response.body, "{}");
}
