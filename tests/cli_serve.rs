use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{self, Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long a test waits for the database file to appear, or for serve to exit.
const DEADLINE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

/// Variables that change how the binary reads its configuration. Every test clears them and
/// then sets the ones it needs, so the outcome does not depend on the shell running the tests.
const HAGGLE_VARS: [&str; 4] = [
    "HAGGLE_DB",
    "HAGGLE_LISTEN",
    "HAGGLE_NTFY_URL",
    "HAGGLE_NTFY_TOKEN",
];

/// A database path in the system temp directory, unique to one test. The file and SQLite's
/// sidecar files are removed when the value is created and again when it is dropped.
struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let path = env::temp_dir().join(format!("haggle-cli-serve-{}-{name}.db", process::id()));
        let db = Self { path };
        db.remove_files();
        db
    }

    fn path_str(&self) -> &str {
        self.path.to_str().expect("temp path is UTF-8")
    }

    fn remove_files(&self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            fs::remove_file(name).ok();
        }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        self.remove_files();
    }
}

/// A running `haggle serve`. It is killed on drop, so a failed test leaves no process behind.
struct ServeProcess {
    child: Child,
}

impl ServeProcess {
    /// Starts serve with its stderr discarded.
    fn start(vars: &[(&str, &str)]) -> Self {
        Self::spawn(vars, Stdio::null())
    }

    /// Starts serve with its stderr piped, so a test can read it with `stderr_text`.
    fn start_capturing_stderr(vars: &[(&str, &str)]) -> Self {
        Self::spawn(vars, Stdio::piped())
    }

    fn spawn(vars: &[(&str, &str)], stderr: Stdio) -> Self {
        let child = command(&["serve"], vars)
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .expect("start haggle serve");
        Self { child }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Waits up to 5 s for the process to exit. Returns its status, or None on timeout.
    fn wait_for_exit(&mut self) -> Option<ExitStatus> {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll haggle serve") {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(POLL);
        }
    }

    /// Returns what serve wrote to stderr. Call it only after the process has exited, because
    /// it reads until the pipe closes.
    fn stderr_text(&mut self) -> String {
        let mut text = String::new();
        if let Some(mut stderr) = self.child.stderr.take() {
            stderr.read_to_string(&mut text).ok();
        }
        text
    }
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        // An error here means the process has already exited, which is what we want.
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// Polls `condition` until it holds or 5 s pass. Returns whether it held.
fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

fn command(args: &[&str], vars: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_haggle"));
    command.args(args);
    for name in HAGGLE_VARS {
        command.env_remove(name);
    }
    command.envs(vars.iter().copied());
    command
}

fn run(args: &[&str], vars: &[(&str, &str)]) -> Output {
    command(args, vars).output().expect("run the haggle binary")
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn send_sigterm(pid: u32) {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("run kill");
    assert!(status.success(), "kill -TERM {pid} failed");
}

/// Returns a `127.0.0.1:<port>` address that was free when this was called. The port is
/// reserved by binding it and released again when the listener is dropped.
fn free_local_address() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a local port");
    listener.local_addr().expect("local address").to_string()
}

/// Sends `GET /` as a raw HTTP/1.0 request and returns the whole response. Returns None if the
/// connection is refused or the response does not finish within 5 s.
fn get_root(address: &str) -> Option<String> {
    let mut stream = TcpStream::connect(address).ok()?;
    stream.set_read_timeout(Some(DEADLINE)).ok()?;
    stream.write_all(b"GET / HTTP/1.0\r\n\r\n").ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

/// Polls `GET /` until it answers 200 OK or 5 s pass. Returns the response if it did.
fn wait_for_root_page(address: &str) -> Option<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(response) = get_root(address).filter(|r| status_code(r) == Some("200")) {
            return Some(response);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(POLL);
    }
}

/// The status code from the status line, such as "200" for "HTTP/1.1 200 OK".
fn status_code(response: &str) -> Option<&str> {
    response.lines().next()?.split_whitespace().nth(1)
}

/// The part of an HTTP response after the blank line that ends the headers.
fn response_body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// Starts serve with the web UI on a free local port, and waits until the dashboard answers.
/// Returns the running process, the address it listens on, and the response to `GET /`.
fn start_serving_web_ui(db: &TempDb) -> (ServeProcess, String, String) {
    let address = free_local_address();
    let serve = ServeProcess::start(&[
        ("HAGGLE_DB", db.path_str()),
        ("HAGGLE_LISTEN", address.as_str()),
    ]);
    let response = wait_for_root_page(&address)
        .unwrap_or_else(|| panic!("GET / on {address} did not return 200 OK within 5 s"));
    (serve, address, response)
}

#[test]
fn serve_without_haggle_db_exits_with_status_two() {
    let output = run(&["serve"], &[]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        stderr_text(&output)
    );
}

#[test]
fn serve_without_haggle_db_names_the_unset_variable_on_stderr() {
    let output = run(&["serve"], &[]);

    assert!(
        stderr_text(&output).contains("HAGGLE_DB is not set"),
        "stderr: {}",
        stderr_text(&output)
    );
}

#[test]
fn configuration_error_is_printed_as_an_error_line_on_stderr() {
    // main prints a configuration error as `error: <message>`.
    let output = run(&["serve"], &[]);

    assert!(
        stderr_text(&output)
            .lines()
            .any(|line| line.starts_with("error: ")),
        "stderr: {}",
        stderr_text(&output)
    );
}

#[test]
fn serve_with_invalid_listen_address_exits_with_status_two() {
    // "nope" is not a socket address, so the configuration is invalid.
    let db = TempDb::new("invalid-listen");

    let output = run(
        &["serve"],
        &[("HAGGLE_DB", db.path_str()), ("HAGGLE_LISTEN", "nope")],
    );

    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        stderr_text(&output)
    );
}

#[test]
fn serve_with_invalid_listen_address_prints_an_error_line_on_stderr() {
    let db = TempDb::new("invalid-listen-message");

    let output = run(
        &["serve"],
        &[("HAGGLE_DB", db.path_str()), ("HAGGLE_LISTEN", "nope")],
    );

    assert!(
        stderr_text(&output)
            .lines()
            .any(|line| line.starts_with("error: ")),
        "stderr: {}",
        stderr_text(&output)
    );
}

#[test]
fn add_without_haggle_db_exits_with_status_two() {
    let output = run(&["add", "https://shop.example/kettle"], &[]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        stderr_text(&output)
    );
}

#[test]
fn add_without_haggle_db_names_the_unset_variable_on_stderr() {
    let output = run(&["add", "https://shop.example/kettle"], &[]);

    assert!(
        stderr_text(&output).contains("HAGGLE_DB is not set"),
        "stderr: {}",
        stderr_text(&output)
    );
}

#[test]
fn serve_exits_with_status_zero_after_sigterm() {
    let db = TempDb::new("sigterm");
    let mut serve = ServeProcess::start(&[("HAGGLE_DB", db.path_str())]);

    let created = wait_until(|| db.path.exists());
    assert!(created, "serve did not create the database within 5 s");
    send_sigterm(serve.pid());

    let status = serve
        .wait_for_exit()
        .expect("serve did not exit within 5 s of SIGTERM");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn serve_answers_get_root_with_200_ok_on_haggle_listen_address() {
    let db = TempDb::new("web-status");

    let (_serve, _address, response) = start_serving_web_ui(&db);

    assert_eq!(status_code(&response), Some("200"), "response: {response}");
}

#[test]
fn serve_serves_the_dashboard_with_watches_heading_on_haggle_listen_address() {
    let db = TempDb::new("web-dashboard");

    let (_serve, _address, response) = start_serving_web_ui(&db);

    assert!(
        response_body(&response).contains("Watches"),
        "body does not contain \"Watches\": {response}"
    );
}

#[test]
fn serve_exits_with_status_zero_after_sigterm_while_web_ui_is_running() {
    let db = TempDb::new("web-sigterm");
    let (mut serve, _address, _response) = start_serving_web_ui(&db);

    send_sigterm(serve.pid());

    let status = serve
        .wait_for_exit()
        .expect("serve did not exit within 5 s of SIGTERM");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn serve_releases_haggle_listen_address_after_sigterm() {
    let db = TempDb::new("web-release");
    let (mut serve, address, _response) = start_serving_web_ui(&db);

    send_sigterm(serve.pid());
    serve
        .wait_for_exit()
        .expect("serve did not exit within 5 s of SIGTERM");

    assert!(
        TcpListener::bind(address.as_str()).is_ok(),
        "{address} is still in use after serve exited"
    );
}

#[test]
fn serve_with_haggle_listen_address_in_use_exits_with_status_one() {
    let db = TempDb::new("busy-listen");
    let busy = TcpListener::bind("127.0.0.1:0").expect("hold a local port");
    let address = busy.local_addr().expect("local address").to_string();
    let mut serve = ServeProcess::start_capturing_stderr(&[
        ("HAGGLE_DB", db.path_str()),
        ("HAGGLE_LISTEN", address.as_str()),
    ]);

    let status = serve
        .wait_for_exit()
        .expect("serve did not exit within 5 s");

    assert_eq!(status.code(), Some(1), "stderr: {}", serve.stderr_text());
}

#[test]
fn serve_with_haggle_listen_address_in_use_names_the_address_on_stderr() {
    let db = TempDb::new("busy-listen-message");
    let busy = TcpListener::bind("127.0.0.1:0").expect("hold a local port");
    let address = busy.local_addr().expect("local address").to_string();
    let mut serve = ServeProcess::start_capturing_stderr(&[
        ("HAGGLE_DB", db.path_str()),
        ("HAGGLE_LISTEN", address.as_str()),
    ]);
    serve
        .wait_for_exit()
        .expect("serve did not exit within 5 s");

    let stderr = serve.stderr_text();

    assert!(
        stderr.contains(&address),
        "stderr does not name {address}: {stderr}"
    );
}
