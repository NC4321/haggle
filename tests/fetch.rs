//! Tests for `haggle::fetch::fetch` and `fetch_page`, run against a local HTTP server on 127.0.0.1.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use haggle::fetch::{
    FetchError, Fetched, Fetcher, Politeness, USER_AGENT, Validators, fetch, fetch_page,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, UnboundedReceiver};
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

/// One hour, the default `rate_limit_pause`.
const HOUR: Duration = Duration::from_secs(3600);

/// The most robots.txt bytes the spec reads.
const ROBOTS_CAP: usize = 500 * 1024;

/// Allowance for the server reading a request a moment after the client sent it.
const READ_SLACK: Duration = Duration::from_millis(20);

/// A fetcher whose politeness is `politeness`.
fn polite_fetcher(politeness: Politeness) -> Fetcher {
    fetcher().with_politeness(politeness)
}

/// Default politeness with host spacing switched off, so tests do not wait.
fn no_spacing() -> Politeness {
    Politeness {
        host_interval: Duration::ZERO,
        ..Politeness::default()
    }
}

/// The URL of `path` on the server whose base URL is `base`.
fn at(base: &Url, path: &str) -> Url {
    base.join(path).expect("valid url")
}

/// A 2xx robots.txt reply carrying `body`.
fn robots_reply(body: &str) -> Vec<u8> {
    respond(
        "200 OK",
        &[&content_length(body.as_bytes())],
        body.as_bytes(),
    )
}

/// A reply with `status` and no body.
fn empty_reply(status: &str) -> Vec<u8> {
    respond(status, &["Content-Length: 0"], b"")
}

/// A 429 reply, with `Retry-After` set to `seconds` when given.
fn rate_limited_reply(retry_after: Option<&str>) -> Vec<u8> {
    match retry_after {
        Some(seconds) => respond(
            "429 Too Many Requests",
            &[&format!("Retry-After: {seconds}"), "Content-Length: 0"],
            b"",
        ),
        None => empty_reply("429 Too Many Requests"),
    }
}

/// Routes `/robots.txt` to `robots` and every other path to `page`.
fn site(robots: Vec<u8>, page: Vec<u8>) -> impl Fn(&str) -> Vec<u8> + Send + 'static {
    move |path: &str| {
        if path == "/robots.txt" {
            robots.clone()
        } else {
            page.clone()
        }
    }
}

/// One request as the recording server read it.
struct Seen {
    /// The request target, such as `/robots.txt`.
    path: String,
    /// When the server read the request head.
    at: Instant,
}

/// Serves every connection until the test ends, answering with `route`. Each request
/// is reported on the returned channel before its reply is written, so once a fetch
/// has returned, every request it made is already queued there.
async fn serve_recorded<F>(route: F) -> (Url, UnboundedReceiver<Seen>)
where
    F: Fn(&str) -> Vec<u8> + Send + 'static,
{
    let listener = bind().await;
    let address = listener.local_addr().expect("local address");
    let (seen_tx, seen_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept connection");
            let head = read_head(&mut socket).await;
            let path = request_path(&head).to_owned();
            let _ = seen_tx.send(Seen {
                path: path.clone(),
                at: Instant::now(),
            });
            let _ = socket.write_all(&route(path.as_str())).await;
            let _ = socket.shutdown().await;
        }
    });
    let base = Url::parse(&format!("http://{address}/")).expect("valid url");
    (base, seen_rx)
}

/// Every request the recording server has read and not yet handed out.
fn drain(seen: &mut UnboundedReceiver<Seen>) -> Vec<Seen> {
    let mut requests = Vec::new();
    while let Ok(request) = seen.try_recv() {
        requests.push(request);
    }
    requests
}

/// How many of `requests` target exactly `path`.
fn count_to(requests: &[Seen], path: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.path == path)
        .count()
}

#[test]
fn politeness_default_is_five_seconds_one_day_and_one_hour() {
    let politeness = Politeness::default();

    assert_eq!(politeness.host_interval, Duration::from_secs(5));
    assert_eq!(politeness.robots_ttl, Duration::from_secs(24 * 3600));
    assert_eq!(politeness.rate_limit_pause, HOUR);
}

#[tokio::test]
async fn page_is_fetched_when_robots_txt_is_missing() {
    let (base, _seen) = serve_recorded(site(empty_reply("404 Not Found"), page_reply())).await;

    let (html, _) =
        expect_body(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert_eq!(html, PAGE);
}

#[tokio::test]
async fn page_is_fetched_when_robots_txt_is_client_error_other_than_404() {
    // Every 4xx for robots.txt allows all, not only 404.
    let (base, _seen) = serve_recorded(site(empty_reply("403 Forbidden"), page_reply())).await;

    let (html, _) =
        expect_body(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert_eq!(html, PAGE);
}

#[tokio::test]
async fn disallow_all_robots_blocks_page_without_requesting_it() {
    let (base, mut seen) = serve_recorded(site(
        robots_reply("User-agent: *\nDisallow: /\n"),
        page_reply(),
    ))
    .await;

    let error =
        expect_error(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert!(
        matches!(error, FetchError::Blocked),
        "expected Blocked, got {error:?}"
    );
    assert_eq!(count_to(&drain(&mut seen), "/page"), 0);
}

#[tokio::test]
async fn robots_disallowing_only_private_lets_public_page_through() {
    let (base, _seen) = serve_recorded(site(
        robots_reply("User-agent: *\nDisallow: /private\n"),
        page_reply(),
    ))
    .await;

    let (html, _) =
        expect_body(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/public"), None).await);

    assert_eq!(html, PAGE);
}

#[tokio::test]
async fn robots_disallowing_only_private_blocks_private_page_without_requesting_it() {
    let (base, mut seen) = serve_recorded(site(
        robots_reply("User-agent: *\nDisallow: /private\n"),
        page_reply(),
    ))
    .await;

    let error =
        expect_error(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/private"), None).await);

    assert!(
        matches!(error, FetchError::Blocked),
        "expected Blocked, got {error:?}"
    );
    assert_eq!(count_to(&drain(&mut seen), "/private"), 0);
}

#[tokio::test]
async fn robots_rule_for_another_agent_does_not_block_page() {
    let (base, _seen) = serve_recorded(site(
        robots_reply("User-agent: otherbot\nDisallow: /\n"),
        page_reply(),
    ))
    .await;

    let (html, _) =
        expect_body(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert_eq!(html, PAGE);
}

#[tokio::test]
async fn robots_disallow_is_matched_against_the_query_string() {
    // `/page?secret=1` starts with the disallowed prefix `/page?secret`.
    let (base, mut seen) = serve_recorded(site(
        robots_reply("User-agent: *\nDisallow: /page?secret\n"),
        page_reply(),
    ))
    .await;

    let error = expect_error(
        fetch_page(
            &polite_fetcher(no_spacing()),
            &at(&base, "/page?secret=1"),
            None,
        )
        .await,
    );

    assert!(
        matches!(error, FetchError::Blocked),
        "expected Blocked, got {error:?}"
    );
    assert_eq!(count_to(&drain(&mut seen), "/page?secret=1"), 0);
}

#[tokio::test]
async fn robots_disallow_within_first_500_kib_blocks_page() {
    // The rule is the first line; 100 KiB of comment follow it, all within the cap.
    let body = format!("User-agent: *\nDisallow: /\n{}", "#".repeat(100 * 1024));
    let (base, _seen) = serve_recorded(site(robots_reply(&body), page_reply())).await;

    let error =
        expect_error(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert!(
        matches!(error, FetchError::Blocked),
        "expected Blocked, got {error:?}"
    );
}

#[tokio::test]
async fn robots_disallow_past_500_kib_is_ignored() {
    // The comment fills past the 500 KiB the spec reads, so the Disallow line after it is
    // never read and the page is allowed.
    let body = format!(
        "{}\nUser-agent: *\nDisallow: /\n",
        "#".repeat(ROBOTS_CAP + 100 * 1024)
    );
    let (base, _seen) = serve_recorded(site(robots_reply(&body), page_reply())).await;

    let (html, _) =
        expect_body(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert_eq!(html, PAGE);
}

#[tokio::test]
async fn robots_server_error_blocks_page_without_requesting_it() {
    let (base, mut seen) =
        serve_recorded(site(empty_reply("500 Internal Server Error"), page_reply())).await;

    let error =
        expect_error(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert!(
        matches!(error, FetchError::Blocked),
        "expected Blocked, got {error:?}"
    );
    assert_eq!(count_to(&drain(&mut seen), "/page"), 0);
}

#[tokio::test]
async fn robots_server_error_is_not_cached() {
    // The first robots.txt request fails with 500; later ones allow everything.
    let robots_requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&robots_requests);
    let (base, mut seen) = serve_recorded(move |path: &str| {
        if path != "/robots.txt" {
            return page_reply();
        }
        match counter.fetch_add(1, Ordering::SeqCst) {
            0 => empty_reply("500 Internal Server Error"),
            _ => robots_reply("User-agent: *\nAllow: /\n"),
        }
    })
    .await;
    let client = polite_fetcher(no_spacing());

    let first = expect_error(fetch_page(&client, &at(&base, "/page"), None).await);
    assert!(
        matches!(first, FetchError::Blocked),
        "expected Blocked, got {first:?}"
    );

    let (html, _) = expect_body(fetch_page(&client, &at(&base, "/page"), None).await);

    assert_eq!(html, PAGE);
    assert_eq!(count_to(&drain(&mut seen), "/robots.txt"), 2);
}

#[tokio::test]
async fn unreachable_origin_gives_network_error_from_robots_fetch() {
    let url = closed_port_url().await;

    let error = expect_error(fetch_page(&polite_fetcher(no_spacing()), &url, None).await);

    assert!(
        matches!(error, FetchError::Network(_)),
        "expected Network, got {error:?}"
    );
}

#[tokio::test]
async fn robots_is_fetched_once_for_two_pages_of_one_origin() {
    let (base, mut seen) = serve_recorded(site(empty_reply("404 Not Found"), page_reply())).await;
    let client = polite_fetcher(no_spacing());

    let _ = expect_body(fetch_page(&client, &at(&base, "/a"), None).await);
    let _ = expect_body(fetch_page(&client, &at(&base, "/b"), None).await);

    assert_eq!(count_to(&drain(&mut seen), "/robots.txt"), 1);
}

#[tokio::test]
async fn page_429_gives_rate_limited() {
    let (base, _seen) =
        serve_recorded(site(empty_reply("404 Not Found"), rate_limited_reply(None))).await;

    let error =
        expect_error(fetch_page(&polite_fetcher(no_spacing()), &at(&base, "/page"), None).await);

    assert!(
        matches!(error, FetchError::RateLimited),
        "expected RateLimited, got {error:?}"
    );
}

#[tokio::test]
async fn paused_host_refuses_another_path_without_a_request() {
    let (base, mut seen) =
        serve_recorded(site(empty_reply("404 Not Found"), rate_limited_reply(None))).await;
    let client = polite_fetcher(no_spacing());
    let _ = fetch_page(&client, &at(&base, "/page"), None).await;
    // Forget the requests the first call made; only the second call is checked.
    let _ = drain(&mut seen);

    let error = expect_error(fetch_page(&client, &at(&base, "/other"), None).await);

    assert!(
        matches!(error, FetchError::RateLimited),
        "expected RateLimited, got {error:?}"
    );
    assert!(
        drain(&mut seen).is_empty(),
        "no request should reach a paused host"
    );
}

#[tokio::test]
async fn paused_until_is_at_least_rate_limit_pause_after_429() {
    let (base, _seen) =
        serve_recorded(site(empty_reply("404 Not Found"), rate_limited_reply(None))).await;
    let client = polite_fetcher(no_spacing());

    let before = Instant::now();
    let _ = fetch_page(&client, &at(&base, "/page"), None).await;

    let until = client
        .paused_until(&at(&base, "/page"))
        .await
        .expect("host should be paused after a 429");
    assert!(
        until >= before + HOUR,
        "paused until {until:?}, expected at least one hour after {before:?}"
    );
}

#[tokio::test]
async fn retry_after_longer_than_pause_sets_the_longer_pause() {
    // Retry-After 7200 s is two hours, longer than the one-hour default pause.
    let (base, _seen) = serve_recorded(site(
        empty_reply("404 Not Found"),
        rate_limited_reply(Some("7200")),
    ))
    .await;
    let client = polite_fetcher(no_spacing());

    let before = Instant::now();
    let _ = fetch_page(&client, &at(&base, "/page"), None).await;

    let until = client
        .paused_until(&at(&base, "/page"))
        .await
        .expect("host should be paused after a 429");
    assert!(
        until >= before + 2 * HOUR,
        "paused until {until:?}, expected at least two hours after {before:?}"
    );
}

#[tokio::test]
async fn retry_after_shorter_than_pause_keeps_the_configured_pause() {
    // Retry-After 10 s is shorter than the one-hour pause, so the pause is still an hour.
    let (base, _seen) = serve_recorded(site(
        empty_reply("404 Not Found"),
        rate_limited_reply(Some("10")),
    ))
    .await;
    let client = polite_fetcher(no_spacing());

    let before = Instant::now();
    let _ = fetch_page(&client, &at(&base, "/page"), None).await;

    let until = client
        .paused_until(&at(&base, "/page"))
        .await
        .expect("host should be paused after a 429");
    assert!(
        until >= before + HOUR,
        "paused until {until:?}, expected at least one hour after {before:?}"
    );
}

#[tokio::test]
async fn host_on_another_port_is_not_paused() {
    let (paused, _paused_seen) =
        serve_recorded(site(empty_reply("404 Not Found"), rate_limited_reply(None))).await;
    let (open, _open_seen) = serve_recorded(site(empty_reply("404 Not Found"), page_reply())).await;
    let client = polite_fetcher(no_spacing());
    let _ = fetch_page(&client, &at(&paused, "/page"), None).await;
    assert!(
        client.paused_until(&at(&paused, "/page")).await.is_some(),
        "the first host should be paused"
    );

    let (html, _) = expect_body(fetch_page(&client, &at(&open, "/page"), None).await);

    assert_eq!(html, PAGE);
    assert!(
        client.paused_until(&at(&open, "/page")).await.is_none(),
        "the second host should not be paused"
    );
}

#[tokio::test]
async fn request_is_sent_again_once_zero_pause_is_over() {
    // With a zero pause the host is free again at once. The first page answers 429 and
    // later ones answer PAGE.
    let page_requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&page_requests);
    let (base, _seen) = serve_recorded(move |path: &str| match path {
        "/robots.txt" => empty_reply("404 Not Found"),
        _ => match counter.fetch_add(1, Ordering::SeqCst) {
            0 => rate_limited_reply(None),
            _ => page_reply(),
        },
    })
    .await;
    let client = polite_fetcher(Politeness {
        rate_limit_pause: Duration::ZERO,
        ..no_spacing()
    });
    let _ = fetch_page(&client, &at(&base, "/page"), None).await;

    let (html, _) = expect_body(fetch_page(&client, &at(&base, "/page"), None).await);

    assert_eq!(html, PAGE);
}

#[tokio::test]
async fn page_requests_to_one_host_are_spaced_by_host_interval() {
    let (base, mut seen) = serve_recorded(site(empty_reply("404 Not Found"), page_reply())).await;
    let client = polite_fetcher(Politeness {
        host_interval: Duration::from_millis(200),
        ..Politeness::default()
    });
    let _ = expect_body(fetch_page(&client, &at(&base, "/a"), None).await);
    let _ = expect_body(fetch_page(&client, &at(&base, "/b"), None).await);

    let page_reads: Vec<Instant> = drain(&mut seen)
        .iter()
        .filter(|request| request.path != "/robots.txt")
        .map(|request| request.at)
        .collect();
    assert_eq!(page_reads.len(), 2, "expected two page requests");
    let gap = page_reads[1].duration_since(page_reads[0]);
    assert!(
        gap + READ_SLACK >= Duration::from_millis(200),
        "page requests {gap:?} apart, expected at least 200 ms"
    );
}

#[tokio::test]
async fn fetch_ignores_robots_and_never_requests_robots_txt() {
    let (base, mut seen) = serve_recorded(site(
        robots_reply("User-agent: *\nDisallow: /\n"),
        page_reply(),
    ))
    .await;

    let (html, _) = expect_body(fetch(&fetcher(), &at(&base, "/page"), None).await);

    assert_eq!(html, PAGE);
    assert_eq!(count_to(&drain(&mut seen), "/robots.txt"), 0);
}
