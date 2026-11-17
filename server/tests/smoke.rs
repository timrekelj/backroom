mod common;

use backroom_server::proto::backroom::v1::{AuthServiceClient, GetMeRequest};
use common::TestApp;
use connectrpc::ErrorCode;

#[tokio::test]
async fn health_and_connect_routing() {
    let app = TestApp::start().await;

    let health = http_get_health(&app.base_url).await;
    assert_eq!(health, "ok");

    let client = app.client(AuthServiceClient::new, None);
    let err = client
        .get_me(GetMeRequest::default())
        .await
        .expect_err("GetMe requires a token");
    assert_eq!(err.code, ErrorCode::Unauthenticated);

    app.cleanup().await;
}

/// Minimal HTTP/1.1 GET /health without pulling in an HTTP client crate.
async fn http_get_health(base_url: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = base_url.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await.unwrap();
    assert!(buf.starts_with("HTTP/1.1 200"), "unexpected response: {buf}");
    buf.split("\r\n\r\n").nth(1).unwrap_or_default().trim().to_owned()
}
