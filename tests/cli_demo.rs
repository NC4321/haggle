use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// How long the dashboard may take to answer `GET /` for the first time.
const STARTUP: Duration = Duration::from_secs(10);
/// How long a timeline event may take to show, counted from when the demo was started.
const TIMELINE: Duration = Duration::from_secs(50);
/// How long the demo may take to print a line, or to exit after SIGTERM or a failure.
const EXIT: Duration = Duration::from_secs(10);
/// How long one HTTP request may take before it counts as failed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the dashboard is checked for a timeline event.
const TIMELINE_POLL: Duration = Duration::from_secs(1);
/// How often a process, a port or a line is polled while a test waits for it.
const POLL: Duration = Duration::from_millis(20);

/// Variables that change how the binary reads its configuration. Every test clears them and
/// then sets the ones it needs, so the outcome does not depend on the shell running the tests.
const CONFIG_VARS: [&str; 5] = [
    "HAGGLE_DB",
    "HAGGLE_LISTEN",
    "GMAIL_USER",
    "GMAIL_APP_PASSWORD",
    "ALERT_EMAIL",
];

/// A running `haggle demo`. Its stdout and stderr are read in the background, so a test can wait
/// for a line with a time limit. It is killed on drop, and the temp database it left behind is
/// removed, so a failed test leaves nothing running or on disk.
struct DemoProcess {
    child: Child,
    stdout: Receiver<String>,
    stderr: Receiver<String>,
}

impl DemoProcess {
    /// Starts `haggle demo` with `HAGGLE_LISTEN` set to `listen`.
    fn start(listen: &str) -> Self {
        let mut child = demo_command(listen)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start haggle demo");
        let stdout = read_lines_in_background(child.stdout.take().expect("piped stdout"));
        let stderr = read_lines_in_background(child.stderr.take().expect("piped stderr"));
        Self {
            child,
            stdout,
            stderr,
        }
    }

    /// Starts `haggle demo` on a local port that was free when this was called. Returns the
    /// process and the address it was asked to listen on.
    fn start_on_free_port() -> (Self, String) {
        let address = free_local_address();
        let demo = Self::start(&address);
        (demo, address)
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The temp database that `demo` creates for this process. The spec names it as the temp
    /// directory joined with `haggle-demo-<process id>.db`.
    fn database_path(&self) -> PathBuf {
        env::temp_dir().join(format!("haggle-demo-{}.db", self.pid()))
    }

    /// Returns the text after `prefix` on the first stdout line that starts with it. Waits up to
    /// 10 s for that line, and panics if it does not come.
    fn stdout_value(&self, prefix: &str) -> String {
        let deadline = Instant::now() + EXIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = self.stdout.recv_timeout(remaining) else {
                panic!("stdout has no line starting with {prefix:?} within 10 s");
            };
            if let Some(value) = line.strip_prefix(prefix) {
                return value.to_string();
            }
        }
    }

    /// Waits up to 10 s for the process to exit. Returns its status, or None on timeout.
    fn wait_for_exit(&mut self) -> Option<ExitStatus> {
        let deadline = Instant::now() + EXIT;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll haggle demo") {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(POLL);
        }
    }

    /// Returns the lines the demo wrote to stderr. Call it only after the process has exited,
    /// because it reads until the pipe closes.
    fn stderr_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(line) = self.stderr.recv_timeout(EXIT) {
            lines.push(line);
        }
        lines
    }
}

impl Drop for DemoProcess {
    fn drop(&mut self) {
        // An error here means the process has already exited, which is what we want.
        self.child.kill().ok();
        self.child.wait().ok();
        for file in database_files(&self.database_path()) {
            fs::remove_file(file).ok();
        }
    }
}

/// The demo's database file and the SQLite sidecar files that go with it.
fn database_files(database: &Path) -> Vec<PathBuf> {
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut name = database.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        })
        .collect()
}

/// The database files from `database_files` that still exist.
fn leftover_database_files(database: &Path) -> Vec<PathBuf> {
    database_files(database)
        .into_iter()
        .filter(|file| file.exists())
        .collect()
}

fn demo_command(listen: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_haggle"));
    command.arg("demo");
    for name in CONFIG_VARS {
        command.env_remove(name);
    }
    command.env("HAGGLE_LISTEN", listen);
    command
}

/// Forwards each line of `pipe` to the returned channel. The channel closes when the pipe does.
fn read_lines_in_background(pipe: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
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

/// Sends `GET <path>` as a raw HTTP/1.0 request and returns the whole response. Returns None if
/// the connection is refused or the response does not finish within `REQUEST_TIMEOUT`.
fn get(address: &str, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(address).ok()?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT)).ok()?;
    write!(stream, "GET {path} HTTP/1.0\r\n\r\n").ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

/// The status code from the status line, such as "200" for "HTTP/1.1 200 OK".
fn status_code(response: &str) -> Option<&str> {
    response.lines().next()?.split_whitespace().nth(1)
}

/// The part of an HTTP response after the blank line that ends the headers.
fn response_body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// The body of the response to `GET <path>` if it is 200 OK, or None if it is not (yet).
fn ok_body(address: &str, path: &str) -> Option<String> {
    let response = get(address, path)?;
    (status_code(&response) == Some("200")).then(|| response_body(&response).to_string())
}

/// Polls `GET <path>` until it answers 200 OK or `within` passes. Returns the body if it did.
fn wait_for_page(address: &str, path: &str, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(body) = ok_body(address, path) {
            return Some(body);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(POLL);
    }
}

/// Waits for `GET <path>` to answer 200 OK within 10 s, and returns its body.
fn expect_page(address: &str, path: &str) -> String {
    wait_for_page(address, path, STARTUP)
        .unwrap_or_else(|| panic!("GET {path} did not answer 200 OK within 10 s"))
}

/// Checks the dashboard once a second until `GET <path>` answers 200 OK with `text` in its body,
/// or until `deadline` passes. Returns whether the text was seen.
fn wait_for_text_on_page(address: &str, path: &str, text: &str, deadline: Instant) -> bool {
    loop {
        if ok_body(address, path).is_some_and(|body| body.contains(text)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(TIMELINE_POLL);
    }
}

#[test]
fn demo_answers_get_root_with_200_ok_within_10_seconds() {
    let (_demo, address) = DemoProcess::start_on_free_port();

    let body = wait_for_page(&address, "/", STARTUP);

    assert!(body.is_some(), "GET / did not answer 200 OK within 10 s");
}

#[test]
fn demo_dashboard_lists_the_three_demo_products() {
    let (_demo, address) = DemoProcess::start_on_free_port();
    let body = expect_page(&address, "/");

    for product in ["Steel kettle", "Wireless headphones", "Countertop blender"] {
        assert!(
            body.contains(product),
            "dashboard does not list {product:?}: {body}"
        );
    }
}

#[test]
fn demo_prints_database_path_on_stdout() {
    let (demo, _address) = DemoProcess::start_on_free_port();

    let printed = demo.stdout_value("database: ");

    let expected = demo.database_path();
    assert_eq!(printed, expected.to_str().expect("temp path is UTF-8"));
}

#[test]
fn demo_prints_dashboard_url_on_stdout() {
    let (demo, address) = DemoProcess::start_on_free_port();

    let printed = demo.stdout_value("dashboard: ");

    assert_eq!(printed, format!("http://{address}/"));
}

#[test]
fn demo_watch_one_chart_has_history_from_the_start() {
    // The chart is drawn from history the demo seeds before it serves the dashboard.
    let (_demo, address) = DemoProcess::start_on_free_port();

    let body = expect_page(&address, "/watches/1");

    assert!(body.contains("<polyline"), "chart has no polyline: {body}");
}

#[test]
fn demo_shows_price_drop_on_watch_one_within_50_seconds() {
    let started = Instant::now();
    let (_demo, address) = DemoProcess::start_on_free_port();

    let shown = wait_for_text_on_page(&address, "/watches/1", "Price drop", started + TIMELINE);

    assert!(
        shown,
        "GET /watches/1 does not contain \"Price drop\" within 50 s of start"
    );
}

#[test]
fn demo_shows_back_in_stock_on_watch_two_within_50_seconds() {
    let started = Instant::now();
    let (_demo, address) = DemoProcess::start_on_free_port();

    let shown = wait_for_text_on_page(&address, "/watches/2", "Back in stock", started + TIMELINE);

    assert!(
        shown,
        "GET /watches/2 does not contain \"Back in stock\" within 50 s of start"
    );
}

#[test]
fn demo_exits_with_status_zero_after_sigterm() {
    let (mut demo, address) = DemoProcess::start_on_free_port();
    // The dashboard is up only after the signals are registered, so the signal is not missed.
    expect_page(&address, "/");

    send_sigterm(demo.pid());

    let status = demo
        .wait_for_exit()
        .expect("demo did not exit within 10 s of SIGTERM");
    assert_eq!(status.code(), Some(0), "stderr: {:?}", demo.stderr_lines());
}

#[test]
fn demo_removes_temp_database_files_after_sigterm() {
    let (mut demo, address) = DemoProcess::start_on_free_port();
    let database = PathBuf::from(demo.stdout_value("database: "));
    expect_page(&address, "/");
    assert!(
        database.exists(),
        "demo did not create {}",
        database.display()
    );

    send_sigterm(demo.pid());
    demo.wait_for_exit()
        .expect("demo did not exit within 10 s of SIGTERM");

    let leftovers = leftover_database_files(&database);
    assert!(leftovers.is_empty(), "files left behind: {leftovers:?}");
}

#[test]
fn demo_with_invalid_listen_address_exits_with_status_two() {
    // "nope" is not a socket address, so the configuration is invalid.
    let mut demo = DemoProcess::start("nope");

    let status = demo.wait_for_exit().expect("demo did not exit within 10 s");

    assert_eq!(status.code(), Some(2), "stderr: {:?}", demo.stderr_lines());
}

#[test]
fn demo_with_invalid_listen_address_prints_an_error_line_on_stderr() {
    // main prints a configuration error as `error: <message>`.
    let mut demo = DemoProcess::start("nope");
    demo.wait_for_exit().expect("demo did not exit within 10 s");

    let stderr = demo.stderr_lines();

    assert!(
        stderr.iter().any(|line| line.starts_with("error: ")),
        "stderr: {stderr:?}"
    );
}

#[test]
fn demo_removes_temp_database_files_when_the_web_ui_cannot_listen() {
    // Another process holds the port, so the web UI cannot start after the database exists.
    let busy = TcpListener::bind("127.0.0.1:0").expect("hold a local port");
    let address = busy.local_addr().expect("local address").to_string();
    let mut demo = DemoProcess::start(&address);

    let status = demo.wait_for_exit().expect("demo did not exit within 10 s");
    assert!(
        !status.success(),
        "demo exited with success while the port was busy"
    );

    let database = PathBuf::from(demo.stdout_value("database: "));
    let leftovers = leftover_database_files(&database);
    assert!(leftovers.is_empty(), "files left behind: {leftovers:?}");
}
