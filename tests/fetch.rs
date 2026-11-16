//! Tests for `haggle::fetch::fetch`, run against a local HTTP server on 127.0.0.1.

use std::time::Duration;

use haggle::fetch::{FetchError, Fetched, Fetcher, USER_AGENT, Validators, fetch};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use url::Url;

/// Timeout for every fetch except the one that must time out.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Body served by the success tests. It is far below every cap used here.
const PAGE: &str = "<html><body><p>price 9.99</p></body></html>";

/// Last hop of the redirect chain: `/hop/N` redirects to `/hop/N+1` until `/hop/6`.
const LAST_HOP: u32 = 6;

/// A fetcher with a cap far above every body used by the tests.
fn fetcher() -> Fetcher {
    Fetcher::new(TIMEOUT, 1_048_576).expect("client should build")
}

/// A fetcher with a 1024-byte body cap.
fn capped_fetcher() -> Fetcher {
    Fetcher::new(TIMEOUT, 1024).expect("client should build")
}

/// Builds a raw HTTP/1.1 reply: the status line, `headers`, `Connection: close`, then `body`.
/// No `Content-Length` is added; a test that needs one passes it in `headers`.
fn respond(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for header in headers {
        head.push_str(header);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut reply = head.into_bytes();
    reply.extend_from_slice(body);
    reply
}

/// The `Content-Length` header line for `body`.
fn content_length(body: &[u8]) -> String {
    format!("Content-Length: {}", body.len())
}

/// A 200 reply carrying `PAGE` and its length.
fn page_reply() -> Vec<u8> {
    respond(
        "200 OK",
        &[&content_length(PAGE.as_bytes())],
        PAGE.as_bytes(),
    )
}

/// Builds validators from the given values, `None` for an absent one.
fn validators(etag: Option<&str>, last_modified: Option<&str>) -> Validators {
    Validators {
        etag: etag.map(str::to_owned),
        last_modified: last_modified.map(str::to_owned),
    }
}

/// Unwraps a fetch that must succeed with a body.
fn expect_body(result: Result<Fetched, FetchError>) -> (String, Validators) {
    match result {
        Ok(Fetched::Body { html, validators }) => (html, validators),
        Ok(Fetched::NotModified) => panic!("expected a body, got NotModified"),
        Err(error) => panic!("expected a body, got {error:?}"),
    }
}

/// Unwraps a fetch that must fail. Returns the error.
fn expect_error(result: Result<Fetched, FetchError>) -> FetchError {
    match result {
        Ok(_) => panic!("expected an error, got a response"),
        Err(error) => error,
    }
}

/// Returns the value of header `name` in a raw request head, matched without case.
fn request_header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Reads the request head, up to and including the blank line that ends it.
async fn read_head(socket: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut chunk = [0u8; 512];
    while !head.ends_with(b"\r\n\r\n") {
        let read = socket.read(&mut chunk).await.expect("read request");
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// The request target of a raw request head, such as `/hop/1`.
fn request_path(head: &str) -> &str {
    head.split_whitespace().nth(1).expect("request target")
}

/// Binds a listener on 127.0.0.1 with a port the OS chooses.
async fn bind() -> TcpListener {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port")
}

/// The URL of `/page` on `listener`.
fn page_url(listener: &TcpListener) -> Url {
    let address = listener.local_addr().expect("local address");
    Url::parse(&format!("http://{address}/page")).expect("valid url")
}

/// Serves one connection with `reply` after reading its request head. The handle yields that head.
async fn serve_once(reply: Vec<u8>) -> (Url, JoinHandle<String>) {
    let listener = bind().await;
    let url = page_url(&listener);
    let request = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept connection");
        let head = read_head(&mut socket).await;
        // The client may stop reading early (over-cap bodies), so write errors are not failures.
        let _ = socket.write_all(&reply).await;
        let _ = socket.shutdown().await;
        head
    });
    (url, request)
}

/// Serves every connection until the test ends. `route` maps a request target to its raw reply.
async fn serve_routes(route: fn(&str) -> Vec<u8>) -> Url {
    let listener = bind().await;
    let address = listener.local_addr().expect("local address");
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept connection");
            let head = read_head(&mut socket).await;
            let _ = socket.write_all(&route(request_path(&head))).await;
            let _ = socket.shutdown().await;
        }
    });
    Url::parse(&format!("http://{address}/")).expect("valid url")
}

/// Accepts one connection and never answers it.
async fn serve_silent() -> Url {
    let listener = bind().await;
    let url = page_url(&listener);
    tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.expect("accept connection");
        std::future::pending::<()>().await;
    });
    url
}

/// A URL whose port has no listener: the port is bound, then released.
async fn closed_port_url() -> Url {
    let listener = bind().await;
    let url = page_url(&listener);
    drop(listener);
    url
}

/// Routes `/hop/N` to `/hop/N+1` until `LAST_HOP`, which answers with `PAGE`.
fn redirect_chain(path: &str) -> Vec<u8> {
    let hop: u32 = path
        .strip_prefix("/hop/")
        .and_then(|number| number.parse().ok())
        .expect("a /hop/N path");
    if hop < LAST_HOP {
        let location = format!("Location: /hop/{}", hop + 1);
        respond("302 Found", &[&location, "Content-Length: 0"], b"")
    } else {
        page_reply()
    }
}

/// Fetches a URL whose server answers with `status` and no body, and returns the error.
async fn error_for_status(status: &str) -> FetchError {
    let (url, _request) = serve_once(respond(status, &["Content-Length: 0"], b"")).await;
    expect_error(fetch(&fetcher(), &url, None).await)
}

#[tokio::test]
async fn returns_body_and_validators_from_200_headers() {
    let reply = respond(
        "200 OK",
        &[
            &content_length(PAGE.as_bytes()),
            r#"ETag: "v1""#,
            "Last-Modified: Wed, 21 Oct 2015 07:28:00 GMT",
        ],
        PAGE.as_bytes(),
    );
    let (url, _request) = serve_once(reply).await;

    let (html, validators) = expect_body(fetch(&fetcher(), &url, None).await);

    assert_eq!(html, PAGE);
    // Validators are the raw header values, quotes included.
    assert_eq!(validators.etag.as_deref(), Some(r#""v1""#));
    assert_eq!(
        validators.last_modified.as_deref(),
        Some("Wed, 21 Oct 2015 07:28:00 GMT")
    );
}

#[tokio::test]
async fn returns_no_validators_when_response_has_no_cache_headers() {
    let (url, _request) = serve_once(page_reply()).await;

    let (_, validators) = expect_body(fetch(&fetcher(), &url, None).await);

    assert_eq!(validators.etag, None);
    assert_eq!(validators.last_modified, None);
}

#[tokio::test]
async fn decodes_body_lossily_as_utf8() {
    // 0xFF never occurs in UTF-8, so it becomes U+FFFD REPLACEMENT CHARACTER.
    let body = *b"ok\xff!";
    let reply = respond("200 OK", &[&content_length(&body)], &body);
    let (url, _request) = serve_once(reply).await;

    let (html, _) = expect_body(fetch(&fetcher(), &url, None).await);

    assert_eq!(html, "ok\u{FFFD}!");
}

#[tokio::test]
async fn body_of_exactly_max_bytes_is_returned() {
    // The cap is 1024 and the limit is "exceeds", so 1024 bytes are accepted.
    let body = [b'a'; 1024];
    let reply = respond("200 OK", &[&content_length(&body)], &body);
    let (url, _request) = serve_once(reply).await;

    let (html, _) = expect_body(fetch(&capped_fetcher(), &url, None).await);

    assert_eq!(html.len(), 1024);
}

#[tokio::test]
async fn sends_get_request_for_the_url_path() {
    let (url, request) = serve_once(page_reply()).await;
    let _ = expect_body(fetch(&fetcher(), &url, None).await);

    let head = request.await.expect("server task");

    assert_eq!(head.lines().next(), Some("GET /page HTTP/1.1"));
}

#[tokio::test]
async fn sends_html_accept_header() {
    let (url, request) = serve_once(page_reply()).await;
    let _ = expect_body(fetch(&fetcher(), &url, None).await);

    let head = request.await.expect("server task");

    assert_eq!(
        request_header(&head, "accept"),
        Some("text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
    );
}

#[tokio::test]
async fn sends_fetcher_user_agent() {
    let (url, request) = serve_once(page_reply()).await;
    let _ = expect_body(fetch(&fetcher(), &url, None).await);

    let head = request.await.expect("server task");

    assert_eq!(request_header(&head, "user-agent"), Some(USER_AGENT));
}

#[tokio::test]
async fn does_not_advertise_gzip() {
    // gzip is not enabled in the client, so no Accept-Encoding header is sent.
    let (url, request) = serve_once(page_reply()).await;
    let _ = expect_body(fetch(&fetcher(), &url, None).await);

    let head = request.await.expect("server task");

    assert_eq!(request_header(&head, "accept-encoding"), None);
}

#[tokio::test]
async fn sends_no_conditional_headers_without_previous_validators() {
    let (url, request) = serve_once(page_reply()).await;
    let _ = expect_body(fetch(&fetcher(), &url, None).await);

    let head = request.await.expect("server task");

    assert_eq!(request_header(&head, "if-none-match"), None);
    assert_eq!(request_header(&head, "if-modified-since"), None);
}

#[tokio::test]
async fn sends_if_none_match_and_if_modified_since_from_previous_validators() {
    let (url, request) = serve_once(page_reply()).await;
    let previous = validators(Some(r#""v1""#), Some("Wed, 21 Oct 2015 07:28:00 GMT"));
    let _ = expect_body(fetch(&fetcher(), &url, Some(&previous)).await);

    let head = request.await.expect("server task");

    assert_eq!(request_header(&head, "if-none-match"), Some(r#""v1""#));
    assert_eq!(
        request_header(&head, "if-modified-since"),
        Some("Wed, 21 Oct 2015 07:28:00 GMT")
    );
}

#[tokio::test]
async fn sends_only_if_none_match_when_last_modified_is_absent() {
    let (url, request) = serve_once(page_reply()).await;
    let previous = validators(Some(r#""v1""#), None);
    let _ = expect_body(fetch(&fetcher(), &url, Some(&previous)).await);

    let head = request.await.expect("server task");

    assert_eq!(request_header(&head, "if-none-match"), Some(r#""v1""#));
    assert_eq!(request_header(&head, "if-modified-since"), None);
}

#[tokio::test]
async fn sends_only_if_modified_since_when_etag_is_absent() {
    let (url, request) = serve_once(page_reply()).await;
    let previous = validators(None, Some("Wed, 21 Oct 2015 07:28:00 GMT"));
    let _ = expect_body(fetch(&fetcher(), &url, Some(&previous)).await);

    let head = request.await.expect("server task");

    assert_eq!(request_header(&head, "if-none-match"), None);
    assert_eq!(
        request_header(&head, "if-modified-since"),
        Some("Wed, 21 Oct 2015 07:28:00 GMT")
    );
}

#[tokio::test]
async fn status_304_returns_not_modified() {
    let reply = respond("304 Not Modified", &["Content-Length: 0"], b"");
    let (url, _request) = serve_once(reply).await;
    let previous = validators(Some(r#""v1""#), None);

    let result = fetch(&fetcher(), &url, Some(&previous)).await;

    assert!(
        matches!(result, Ok(Fetched::NotModified)),
        "expected NotModified"
    );
}

#[tokio::test]
async fn status_403_is_blocked() {
    let error = error_for_status("403 Forbidden").await;

    assert!(
        matches!(error, FetchError::Blocked),
        "expected Blocked, got {error:?}"
    );
}

#[tokio::test]
async fn status_429_is_rate_limited() {
    let error = error_for_status("429 Too Many Requests").await;

    assert!(
        matches!(error, FetchError::RateLimited),
        "expected RateLimited, got {error:?}"
    );
}

#[tokio::test]
async fn status_500_is_http_500() {
    let error = error_for_status("500 Internal Server Error").await;

    assert!(
        matches!(error, FetchError::Http(500)),
        "expected Http(500), got {error:?}"
    );
}

#[tokio::test]
async fn status_404_is_http_404() {
    let error = error_for_status("404 Not Found").await;

    assert!(
        matches!(error, FetchError::Http(404)),
        "expected Http(404), got {error:?}"
    );
}

#[tokio::test]
async fn declared_length_over_max_is_too_large() {
    let body = [b'x'; 4096];
    let reply = respond("200 OK", &[&content_length(&body)], &body);
    let (url, _request) = serve_once(reply).await;

    let error = expect_error(fetch(&capped_fetcher(), &url, None).await);

    assert!(
        matches!(error, FetchError::TooLarge),
        "expected TooLarge, got {error:?}"
    );
}

#[tokio::test]
async fn declared_length_over_max_is_too_large_before_reading_body() {
    // The reply declares 4096 bytes but sends only 5 and closes. Returning TooLarge
    // shows the length was judged from the header; reading the body would have failed.
    let reply = respond("200 OK", &["Content-Length: 4096"], b"short");
    let (url, _request) = serve_once(reply).await;

    let error = expect_error(fetch(&capped_fetcher(), &url, None).await);

    assert!(
        matches!(error, FetchError::TooLarge),
        "expected TooLarge, got {error:?}"
    );
}

#[tokio::test]
async fn body_without_content_length_over_max_is_too_large() {
    let body = [b'x'; 4096];
    let reply = respond("200 OK", &[], &body);
    let (url, _request) = serve_once(reply).await;

    let error = expect_error(fetch(&capped_fetcher(), &url, None).await);

    assert!(
        matches!(error, FetchError::TooLarge),
        "expected TooLarge, got {error:?}"
    );
}

#[tokio::test]
async fn server_that_never_answers_gives_network_error_at_timeout() {
    let url = serve_silent().await;
    let fetcher = Fetcher::new(Duration::from_secs(1), 1024).expect("client should build");

    let error = expect_error(fetch(&fetcher, &url, None).await);

    assert!(
        matches!(error, FetchError::Network(_)),
        "expected Network, got {error:?}"
    );
}

#[tokio::test]
async fn refused_connection_gives_network_error() {
    let url = closed_port_url().await;

    let error = expect_error(fetch(&fetcher(), &url, None).await);

    assert!(
        matches!(error, FetchError::Network(_)),
        "expected Network, got {error:?}"
    );
}

#[tokio::test]
async fn five_redirects_are_followed() {
    let base = serve_routes(redirect_chain).await;
    let start = base.join("/hop/1").expect("valid url");

    // From /hop/1 the chain reaches /hop/6 after five redirects, which the limit allows.
    let (html, _) = expect_body(fetch(&fetcher(), &start, None).await);

    assert_eq!(html, PAGE);
}

#[tokio::test]
async fn sixth_redirect_gives_network_error() {
    let base = serve_routes(redirect_chain).await;
    let start = base.join("/hop/0").expect("valid url");

    // From /hop/0 reaching /hop/6 takes six redirects, one more than the limit of five.
    let error = expect_error(fetch(&fetcher(), &start, None).await);

    assert!(
        matches!(error, FetchError::Network(_)),
        "expected Network, got {error:?}"
    );
}
