use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;

const JSON_LD_PRODUCT: &str = include_str!("fixtures/cli/jsonld_product.html");
const META_LIST_PRICE: &str = include_str!("fixtures/cli/meta_list_price.html");
const PLAIN_SELECTOR: &str = include_str!("fixtures/cli/plain_selector.html");
const NO_PRICE: &str = include_str!("fixtures/cli/no_price.html");

const OK: &str = "HTTP/1.1 200 OK";
const FORBIDDEN: &str = "HTTP/1.1 403 Forbidden";
const NOT_MODIFIED: &str = "HTTP/1.1 304 Not Modified";

/// A page served on 127.0.0.1. Every connection gets the same canned answer,
/// and the request head of each connection is recorded.
struct PageServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

fn read_request_head(stream: &TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the connection"));
    let mut head = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if line == "\r\n" => break,
            Ok(_) => head.push_str(&line),
        }
    }
    head
}

fn serve(status_line: &'static str, body: &'static str) -> PageServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let port = listener.local_addr().expect("local address").port();
    let response = format!(
        "{status_line}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else {
                continue;
            };
            let head = read_request_head(&stream);
            log.lock().expect("request log").push(head);
            stream.write_all(response.as_bytes()).ok();
        }
    });
    PageServer {
        url: format!("http://127.0.0.1:{port}/product.html"),
        requests,
    }
}

fn serve_page(body: &'static str) -> PageServer {
    serve(OK, body)
}

fn haggle(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_haggle"))
        .args(args)
        .output()
        .expect("run the haggle binary")
}

fn check(url: &str) -> Output {
    haggle(&["check", url])
}

fn exit_code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the process exited with a code")
}

fn stdout_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn has_line(output: &Output, expected: &str) -> bool {
    stdout_lines(output).iter().any(|line| line == expected)
}

#[test]
fn json_ld_page_prints_price_availability_title_and_source_in_order() {
    // 1299.00 EUR, in stock, title from the Product name, source json-ld.
    let page = serve_page(JSON_LD_PRODUCT);

    let output = check(&page.url);

    assert_eq!(
        stdout_lines(&output),
        [
            "price: 1299.00 EUR",
            "availability: in stock",
            "title: Example Phone",
            "source: json-ld",
        ]
    );
}

#[test]
fn json_ld_page_exits_with_status_zero() {
    let page = serve_page(JSON_LD_PRODUCT);

    let output = check(&page.url);

    assert_eq!(exit_code(&output), 0);
}

#[test]
fn json_ld_page_writes_nothing_to_stderr() {
    let page = serve_page(JSON_LD_PRODUCT);

    let output = check(&page.url);

    assert_eq!(stderr_text(&output), "");
}

#[test]
fn check_leaves_no_files_in_the_working_directory() {
    // Storing nothing: the check runs in an empty directory and must leave it empty.
    let dir = env::temp_dir().join(format!("haggle-cli-check-empty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create an empty working directory");
    let page = serve_page(JSON_LD_PRODUCT);

    let output = Command::new(env!("CARGO_BIN_EXE_haggle"))
        .args(["check", &page.url])
        .current_dir(&dir)
        .output()
        .expect("run the haggle binary");
    let entries = fs::read_dir(&dir)
        .expect("list the working directory")
        .count();
    fs::remove_dir_all(&dir).expect("remove the working directory");

    assert_eq!(exit_code(&output), 0);
    assert_eq!(entries, 0);
}

#[test]
fn check_fetches_the_page_exactly_once() {
    let page = serve_page(JSON_LD_PRODUCT);

    let output = check(&page.url);

    assert_eq!(exit_code(&output), 0);
    let requests = page.requests.lock().expect("request log");
    let page_fetches = requests
        .iter()
        .filter(|head| head.starts_with("GET /product.html "))
        .count();
    assert_eq!(page_fetches, 1);
}

#[test]
fn check_sends_no_cache_validators() {
    // The fetch is unconditional: no If-None-Match or If-Modified-Since header.
    let page = serve_page(JSON_LD_PRODUCT);

    let output = check(&page.url);

    assert_eq!(exit_code(&output), 0);
    let requests = page.requests.lock().expect("request log");
    assert!(!requests.is_empty(), "the page was requested");
    let sends_validator = requests.iter().any(|head| {
        let head = head.to_ascii_lowercase();
        head.contains("if-none-match") || head.contains("if-modified-since")
    });
    assert!(!sends_validator);
}

#[test]
fn meta_page_with_original_price_prints_price_line() {
    let page = serve_page(META_LIST_PRICE);

    let output = check(&page.url);

    assert!(has_line(&output, "price: 1299.00 EUR"));
}

#[test]
fn meta_page_with_original_price_prints_list_price_line() {
    // product:original_price:amount 1499.00 EUR is the shop's "was" price.
    let page = serve_page(META_LIST_PRICE);

    let output = check(&page.url);

    assert!(has_line(&output, "list price: 1499.00 EUR"));
}

#[test]
fn list_price_line_follows_the_price_line() {
    let page = serve_page(META_LIST_PRICE);

    let output = check(&page.url);

    let lines = stdout_lines(&output);
    let price = lines
        .iter()
        .position(|line| line == "price: 1299.00 EUR")
        .expect("a price line");
    let list_price = lines
        .iter()
        .position(|line| line == "list price: 1499.00 EUR")
        .expect("a list price line");
    assert_eq!(list_price, price + 1);
}

#[test]
fn selector_option_prints_price_from_the_selected_element() {
    let page = serve_page(PLAIN_SELECTOR);

    let output = Command::new(env!("CARGO_BIN_EXE_haggle"))
        .args(["check", &page.url, "--selector", ".price"])
        .output()
        .expect("run the haggle binary");

    assert_eq!(exit_code(&output), 0);
    assert!(has_line(&output, "price: 1299.00 EUR"));
}

#[test]
fn selector_option_prints_source_selector() {
    let page = serve_page(PLAIN_SELECTOR);

    let output = Command::new(env!("CARGO_BIN_EXE_haggle"))
        .args(["check", &page.url, "--selector", ".price"])
        .output()
        .expect("run the haggle binary");

    assert!(has_line(&output, "source: selector"));
}

#[test]
fn page_without_price_exits_with_status_one() {
    let page = serve_page(NO_PRICE);

    let output = check(&page.url);

    assert_eq!(exit_code(&output), 1);
}

#[test]
fn page_without_price_writes_error_mentioning_no_price_to_stderr() {
    let page = serve_page(NO_PRICE);

    let output = check(&page.url);

    let stderr = stderr_text(&output);
    assert!(stderr.starts_with("error: "), "stderr was {stderr:?}");
    assert!(
        stderr.to_lowercase().contains("no price"),
        "stderr was {stderr:?}"
    );
}

#[test]
fn page_without_price_prints_nothing_to_stdout() {
    let page = serve_page(NO_PRICE);

    let output = check(&page.url);

    assert_eq!(stdout_lines(&output), Vec::<String>::new());
}

#[test]
fn invalid_selector_on_page_without_price_exits_with_status_one() {
    let page = serve_page(NO_PRICE);

    let output = Command::new(env!("CARGO_BIN_EXE_haggle"))
        .args(["check", &page.url, "--selector", "p[["])
        .output()
        .expect("run the haggle binary");

    assert_eq!(exit_code(&output), 1);
    assert!(stderr_text(&output).starts_with("error: "));
}

#[test]
fn forbidden_answer_exits_with_status_one() {
    let page = serve(FORBIDDEN, NO_PRICE);

    let output = check(&page.url);

    assert_eq!(exit_code(&output), 1);
}

#[test]
fn forbidden_answer_writes_error_to_stderr() {
    let page = serve(FORBIDDEN, NO_PRICE);

    let output = check(&page.url);

    assert!(stderr_text(&output).starts_with("error: "));
}

#[test]
fn not_modified_answer_exits_with_status_one() {
    let page = serve(NOT_MODIFIED, "");

    let output = check(&page.url);

    assert_eq!(exit_code(&output), 1);
}

#[test]
fn not_modified_answer_is_reported_as_unexpected_on_stderr() {
    // An unconditional request must not get 304; the CLI says so with this exact message.
    let page = serve(NOT_MODIFIED, "");

    let output = check(&page.url);

    assert_eq!(
        stderr_text(&output).trim_end(),
        "error: the server answered 304 Not Modified to an unconditional request"
    );
}

#[test]
fn not_a_url_exits_with_status_one() {
    let output = check("not-a-url");

    assert_eq!(exit_code(&output), 1);
}

#[test]
fn not_a_url_error_names_the_invalid_url_on_stderr() {
    let output = check("not-a-url");

    assert!(stderr_text(&output).starts_with("error: invalid URL not-a-url: "));
}

#[test]
fn not_a_url_prints_nothing_to_stdout() {
    let output = check("not-a-url");

    assert_eq!(stdout_lines(&output), Vec::<String>::new());
}

#[test]
fn non_http_scheme_is_rejected_as_invalid_url() {
    // Only http and https are accepted; ftp is an invalid URL for this command.
    let output = check("ftp://example.com/phone");

    assert_eq!(exit_code(&output), 1);
    assert!(stderr_text(&output).starts_with("error: invalid URL ftp://example.com/phone: "));
}

#[test]
fn check_without_url_exits_with_usage_status_two() {
    let output = haggle(&["check"]);

    assert_eq!(exit_code(&output), 2);
}

#[test]
fn log_level_flag_before_the_subcommand_leaves_findings_unchanged() {
    let page = serve_page(JSON_LD_PRODUCT);

    let output = haggle(&["--log-level", "debug", "check", &page.url]);

    assert_eq!(exit_code(&output), 0);
    assert!(has_line(&output, "price: 1299.00 EUR"));
    assert!(has_line(&output, "source: json-ld"));
}

#[test]
fn log_level_flag_after_the_subcommand_leaves_findings_unchanged() {
    let page = serve_page(JSON_LD_PRODUCT);

    let output = haggle(&["check", &page.url, "--log-level", "debug"]);

    assert_eq!(exit_code(&output), 0);
    assert!(has_line(&output, "price: 1299.00 EUR"));
    assert!(has_line(&output, "source: json-ld"));
}
