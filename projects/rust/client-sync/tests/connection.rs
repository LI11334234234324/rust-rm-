use reqwest::{Method, blocking::Client};
use rm_client_sync::exchange;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

#[test]
fn sends_http_authorization_and_preserves_error_status() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        assert!(headers.starts_with("GET /texts HTTP/1.1\r\n"));
        assert!(
            headers
                .to_lowercase()
                .contains("authorization: bearer sample\r\n")
        );
        stream
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\nConnection: close\r\n\r\nexpired",
            )
            .unwrap();
    });
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let result = exchange(&client, &url, Method::GET, "/texts", "sample", None).unwrap();
    assert_eq!(result, (401, json!({"message":"expired"})));
    peer.join().unwrap();
}

#[test]
fn preserves_status_with_non_json_500_error_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 16\r\nConnection: close\r\n\r\nserver breakdown",
            )
            .unwrap();
    });
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let spec = rm_client_sync::build_echo_request("hello");
    let result = exchange(
        &client,
        &url,
        spec.method,
        &spec.path,
        "",
        spec.body.as_ref(),
    )
    .unwrap();
    assert_eq!(result, (500, json!({"message": "server breakdown"})));
    peer.join().unwrap();
}

#[test]
fn exchange_reports_error_when_server_unreachable() {
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap();
    let spec = rm_client_sync::build_ping_request();
    // Port 1 on 127.0.0.1 is not listening
    let result = exchange(
        &client,
        "http://127.0.0.1:1",
        spec.method,
        &spec.path,
        "",
        spec.body.as_ref(),
    );
    assert!(result.is_err());
}
