//! Tests for the `add`, `ls`, `history` and `rm` subcommands, run against the built binary.

use rusqlite::{Connection, params};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};

const URL: &str = "https://shop.example/kettle";
/// 2026-01-02 03:04 UTC in Unix seconds: 20455 whole days after 1970-01-01, plus 03:04 (11040 s).
const JAN_2_0304_UTC: i64 = 1_767_323_040;
/// 2026-01-03 04:05 UTC in Unix seconds: 20456 whole days after 1970-01-01, plus 04:05 (14700 s).
const JAN_3_0405_UTC: i64 = 1_767_413_100;

/// A database file under the temp dir. The file and its WAL side files are removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Names the file after this process and `test`, and clears any leftover from an earlier run.
    fn new(test: &str) -> Self {
        let path = env::temp_dir().join(format!(
            "haggle-cli-watches-{}-{test}.sqlite",
            process::id()
        ));
        let scratch = Scratch { path };
        scratch.remove_files();
        scratch
    }

    fn remove_files(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            let _ = fs::remove_file(PathBuf::from(name));
        }
    }

    fn connection(&self) -> Connection {
        Connection::open(&self.path).expect("open the scratch database")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove_files();
    }
}

/// Runs the binary with `HAGGLE_DB` set to `db`, or with it unset when `db` is `None`.
fn run(db: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_haggle"));
    command.args(args);
    match db {
        Some(path) => command.env("HAGGLE_DB", path),
        None => command.env_remove("HAGGLE_DB"),
    };
    command.output().expect("run the haggle binary")
}

fn run_in(scratch: &Scratch, args: &[&str]) -> Output {
    run(Some(&scratch.path), args)
}

/// Runs `add URL` followed by `extra` arguments.
fn add(scratch: &Scratch, extra: &[&str]) -> Output {
    let mut args = vec!["add", URL];
    args.extend_from_slice(extra);
    run_in(scratch, &args)
}

/// Adds the test watch, which gets id 1 in a fresh database, and checks that it was added.
fn add_watch(scratch: &Scratch) {
    let output = add(scratch, &["--every", "1d"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
}

/// Stores an observation of watch 1 directly in the database, the way the checker would.
fn insert_observation(scratch: &Scratch, at: i64, price_minor: i64, list_price_minor: Option<i64>) {
    scratch
        .connection()
        .execute(
            "INSERT INTO observations \
             (watch_id, at, price_minor, list_price_minor, currency, availability, source) \
             VALUES (1, ?1, ?2, ?3, 'EUR', 'in_stock', 'json_ld')",
            params![at, price_minor, list_price_minor],
        )
        .expect("insert observation");
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

/// Asserts that the run exited with status 1 and that stderr names `needle`.
fn assert_error_naming(output: &Output, needle: &str) {
    let stderr = stderr_text(output);
    assert_eq!(exit_code(output), 1, "exit code, stderr: {stderr}");
    assert!(
        stderr.contains(needle),
        "stderr should name {needle:?}, got {stderr:?}"
    );
}

#[test]
fn add_prints_added_watch_id_and_exits_zero() {
    let scratch = Scratch::new("add_prints_added_watch_id");

    let output = add(&scratch, &["--every", "1d"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["added watch 1"]);
}

#[test]
fn ls_lists_the_new_watch_with_status_ok_no_price_and_url() {
    let scratch = Scratch::new("ls_lists_new_watch");
    add_watch(&scratch);

    let output = run_in(&scratch, &["ls"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    // Columns are separated by two spaces; "-" stands for the missing latest price.
    assert_eq!(
        stdout_lines(&output),
        ["1  ok  -  https://shop.example/kettle"]
    );
}

#[test]
fn ls_shows_the_name_instead_of_the_url_when_one_was_given() {
    let scratch = Scratch::new("ls_shows_name");
    let output = add(&scratch, &["--name", "Kettle", "--every", "1d"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));

    let output = run_in(&scratch, &["ls"]);

    assert_eq!(stdout_lines(&output), ["1  ok  -  Kettle"]);
}

#[test]
fn ls_shows_the_price_of_the_latest_observation() {
    let scratch = Scratch::new("ls_shows_latest_price");
    add_watch(&scratch);
    // The newer observation is inserted first, so the latest price is 18.99 EUR, not the last row inserted.
    insert_observation(&scratch, JAN_3_0405_UTC, 1899, None);
    insert_observation(&scratch, JAN_2_0304_UTC, 1999, None);

    let output = run_in(&scratch, &["ls"]);

    assert_eq!(
        stdout_lines(&output),
        ["1  ok  18.99 EUR  https://shop.example/kettle"]
    );
}

#[test]
fn ls_on_an_empty_database_prints_no_watches() {
    let scratch = Scratch::new("ls_empty");

    let output = run_in(&scratch, &["ls"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["no watches"]);
}

#[test]
fn add_accepts_a_target_that_names_its_currency_code() {
    let scratch = Scratch::new("add_target_code");

    let output = add(&scratch, &["--target", "199.99 EUR"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["added watch 1"]);
}

#[test]
fn add_accepts_a_target_with_a_euro_sign() {
    let scratch = Scratch::new("add_target_euro_sign");

    let output = add(&scratch, &["--target", "€199.99"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["added watch 1"]);
}

#[test]
fn add_target_without_a_currency_fails_and_names_the_currency() {
    let scratch = Scratch::new("add_target_no_currency");

    let output = add(&scratch, &["--target", "199.99"]);

    // The reason is "name the currency, for example 199.99 EUR", and the bad value is echoed back.
    assert_error_naming(&output, "currency");
    assert!(stderr_text(&output).contains("199.99"));
}

#[test]
fn add_with_a_target_missing_its_currency_stores_no_watch() {
    let scratch = Scratch::new("add_target_no_currency_stores_nothing");
    let output = add(&scratch, &["--target", "199.99"]);
    assert_eq!(exit_code(&output), 1);

    let output = run_in(&scratch, &["ls"]);

    assert_eq!(stdout_lines(&output), ["no watches"]);
}

#[test]
fn add_accepts_every_one_day() {
    let scratch = Scratch::new("add_every_1d");

    let output = add(&scratch, &["--every", "1d"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["added watch 1"]);
}

#[test]
fn add_accepts_every_five_minutes_as_the_minimum() {
    let scratch = Scratch::new("add_every_5m");

    let output = add(&scratch, &["--every", "5m"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["added watch 1"]);
}

#[test]
fn add_rejects_every_below_five_minutes_and_names_the_interval() {
    let scratch = Scratch::new("add_every_2m");

    let output = add(&scratch, &["--every", "2m"]);

    assert_error_naming(&output, "2m");
}

#[test]
fn add_rejects_every_of_299_seconds_as_below_the_minimum() {
    // 299 s is 4 min 59 s, one second under the 5-minute minimum.
    let scratch = Scratch::new("add_every_299s");

    let output = add(&scratch, &["--every", "299s"]);

    assert_error_naming(&output, "299s");
}

#[test]
fn add_rejects_every_with_an_unknown_unit_and_names_the_interval() {
    let scratch = Scratch::new("add_every_6x");

    let output = add(&scratch, &["--every", "6x"]);

    assert_error_naming(&output, "6x");
}

#[test]
fn add_accepts_a_selector() {
    let scratch = Scratch::new("add_selector");

    let output = add(&scratch, &["--selector", ".price"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["added watch 1"]);
}

#[test]
fn add_same_url_twice_fails_and_names_the_url() {
    let scratch = Scratch::new("add_duplicate_url");
    add_watch(&scratch);

    let output = add(&scratch, &["--every", "1d"]);

    assert_error_naming(&output, URL);
}

#[test]
fn add_ftp_url_fails_and_names_the_url() {
    let scratch = Scratch::new("add_ftp_url");

    let output = run_in(&scratch, &["add", "ftp://shop.example/kettle"]);

    assert_error_naming(&output, "ftp://shop.example/kettle");
}

#[test]
fn history_prints_observations_oldest_first() {
    let scratch = Scratch::new("history_oldest_first");
    add_watch(&scratch);
    // Inserted newest first on purpose.
    insert_observation(&scratch, JAN_3_0405_UTC, 1899, None);
    insert_observation(&scratch, JAN_2_0304_UTC, 1999, None);

    let output = run_in(&scratch, &["history", "1"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(
        stdout_lines(&output),
        [
            "2026-01-02 03:04 UTC  19.99 EUR  in stock",
            "2026-01-03 04:05 UTC  18.99 EUR  in stock",
        ]
    );
}

#[test]
fn history_appends_the_list_price_when_the_observation_has_one() {
    let scratch = Scratch::new("history_list_price");
    add_watch(&scratch);
    insert_observation(&scratch, JAN_2_0304_UTC, 1999, Some(2499));

    let output = run_in(&scratch, &["history", "1"]);

    assert_eq!(
        stdout_lines(&output),
        ["2026-01-02 03:04 UTC  19.99 EUR  in stock  (list 24.99 EUR)"]
    );
}

#[test]
fn history_of_an_unknown_watch_fails_and_names_the_id() {
    let scratch = Scratch::new("history_unknown_id");
    add_watch(&scratch);

    let output = run_in(&scratch, &["history", "99"]);

    assert_error_naming(&output, "99");
}

#[test]
fn rm_prints_the_removed_watch_id() {
    let scratch = Scratch::new("rm_prints_id");
    add_watch(&scratch);

    let output = run_in(&scratch, &["rm", "1"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));
    assert_eq!(stdout_lines(&output), ["removed watch 1"]);
}

#[test]
fn rm_removes_the_watch_so_ls_then_prints_no_watches() {
    let scratch = Scratch::new("rm_then_ls");
    add_watch(&scratch);
    let output = run_in(&scratch, &["rm", "1"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr_text(&output));

    let output = run_in(&scratch, &["ls"]);

    assert_eq!(stdout_lines(&output), ["no watches"]);
}

#[test]
fn rm_of_an_unknown_watch_fails_and_names_the_id() {
    let scratch = Scratch::new("rm_unknown_id");
    add_watch(&scratch);

    let output = run_in(&scratch, &["rm", "99"]);

    assert_error_naming(&output, "99");
}

#[test]
// Fails until the exit-code mapping for configuration errors (the next card) is in place.
fn without_haggle_db_the_command_exits_two_and_says_it_is_not_set() {
    let output = run(None, &["ls"]);

    assert_eq!(exit_code(&output), 2, "stderr: {}", stderr_text(&output));
    assert!(
        stderr_text(&output).contains("error: HAGGLE_DB is not set"),
        "stderr: {}",
        stderr_text(&output)
    );
}
