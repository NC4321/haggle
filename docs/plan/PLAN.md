# Haggle — Plan

## Goal

A self-hosted price and stock watcher. You give it a product URL; it checks the page on a schedule, keeps the price and availability history, and sends a notification when the price drops, hits a target, or the item goes out of stock or comes back. A small web dashboard shows every watched item with its price chart. It runs as one binary on the homelab server and is used for real purchases.

## Decisions

- Purpose? → GitHub portfolio piece (full-stack product) and a tool for daily personal use: buying homelab, hobby and work items at their lowest price.
- Language? → Rust (stable, edition 2024). One self-contained binary (SQLite compiled in); the server needs the Rust toolchain to build it and nothing extra to run it.
- Host? → `hp-server` (hostname `apollo`, 192.168.5.21, Ubuntu 24.04, x86-64, glibc 2.39), as a systemd service running as user `vette`.
- Database? → SQLite, one file at `/mnt/database/haggle/haggle.db` on hp-server. This is the only persistent database. The folder is created once by hand (`sudo mkdir /mnt/database/haggle && sudo chown vette:vette /mnt/database/haggle`). Haggle never reads or writes any other folder under `/mnt/database`.
- Test and demo data? → throwaway SQLite files in a temp directory, deleted afterwards. Tests and `make demo` never open the live database, and refuse to start if `HAGGLE_DB` points under `/mnt/database`.
- How is a price found? → in order: schema.org JSON-LD (`Product` / `Offer`), then Open Graph and `product:price:*` meta tags, then schema.org microdata (`itemprop="price"`), then a CSS selector the user supplies for that watch. The first source that yields a valid price wins and is recorded with the observation.
- JavaScript-rendered pages? → not supported. No headless browser. A watch whose page yields no price is marked `needs-selector` and shown as such.
- Money? → integer minor units plus an ISO 4217 currency code. Never floating point.
- Politeness? → honour robots.txt; one request at a time per host with a minimum gap; random jitter on every schedule; conditional GET (ETag / Last-Modified); exponential backoff after errors and after HTTP 429 or 503; an honest User-Agent naming the project. Default check interval 6 hours, minimum 15 minutes.
- Notifications? → ntfy (a topic URL, optional bearer token). One notification per event, never repeated for the same event.
- Web UI? → server-rendered HTML with htmx for the few interactive parts. Charts are SVG drawn on the server. No Node toolchain and no JavaScript build.
- Access control? → none. The service binds to the LAN address and is not exposed to the internet.
- Port? → 8421 (22, 80, 3000, 4747, 8000, 8080 and 8787 are already in use on hp-server).
- Build and deploy? → built on hp-server itself, from a git checkout at `~/Projects/haggle`, with a normal glibc release build (`x86_64-unknown-linux-gnu`). It is built against the server's own glibc 2.39, so no musl target and no copying between machines. Installed to `/usr/local/bin/haggle`, run by systemd.
- Server toolchain? → `build-essential` and `git` from apt; Rust stable through rustup, installed for user `vette` in `~/.cargo` (Ubuntu's apt `rustc` is too old for edition 2024).
- Workflow? → solo.
- CI? → none. No workflow files.

## Architecture

- Stack: Rust stable, tokio, axum, reqwest (rustls, no OpenSSL), scraper, rusqlite (bundled SQLite), askama templates, clap, serde, thiserror, tracing.
- Folder layout:
  - `Cargo.toml`, `Makefile`, `rustfmt.toml`, `.gitignore`
  - `src/`: the single crate (library `haggle` + binary `haggle`).
  - `migrations/`: numbered `.sql` files, embedded in the binary.
  - `templates/`: askama HTML templates; `static/`: one CSS file and the vendored htmx script.
  - `tests/`: integration tests; `tests/fixtures/`: saved product pages (HTML) with the expected price beside each.
  - `deploy/`: `haggle.service`, `haggle-backup.service`, `haggle-backup.timer`, `README.md`.
  - `docs/plan/PLAN.md`, `docs/design/` (extraction, scheduling), `media/` (README screenshots and GIF).
- Dependency direction: `web` and `cli` → `scheduler` → `fetch`, `extract`, `rules`, `notify` → `db`. `extract` and `rules` are pure (no I/O) and depend on nothing but `money`.

### Modules

- `src/main.rs`: entry point, error boundary, exit codes.
- `src/cli.rs`: clap definitions and the handlers for each subcommand.
- `src/config.rs`: environment and flags into one `Config`; validation.
- `src/money.rs`: `Money { minor: i64, currency: Currency }`; parsing of price strings ("1.299,00 €", "$1,299.00", "1299").
- `src/extract/`: `mod.rs` (the ordered chain), `jsonld.rs`, `meta.rs`, `microdata.rs`, `selector.rs`. Input is HTML text; output is an `Extraction`. No network.
- `src/fetch.rs`: the HTTP client, robots.txt cache, per-host limiter, conditional GET, backoff.
- `src/rules.rs`: compares a new observation with the watch's history and returns events. Pure.
- `src/notify.rs`: ntfy client.
- `src/scheduler.rs`: picks due watches, runs fetch → extract → store → rules → notify.
- `src/db/`: `mod.rs` (open, pragmas, migrations), `watches.rs`, `observations.rs`, `events.rs`.
- `src/web/`: `mod.rs` (router), `pages.rs` (dashboard, watch detail, add form), `chart.rs` (price history as SVG).
- `src/demo_shop.rs`: a fake shop served on localhost whose prices and stock follow a scripted timeline.

### Interfaces

```rust
pub struct Money { pub minor: i64, pub currency: Currency }
pub enum Availability { InStock, OutOfStock, Preorder, Unknown }
pub enum Source { JsonLd, Meta, Microdata, Selector }

pub struct Extraction {
    pub price: Money,
    pub list_price: Option<Money>,   // the shop's own "was" price, if it states one
    pub availability: Availability,
    pub title: Option<String>,
    pub source: Source,
}
pub fn extract(html: &str, selector: Option<&str>) -> Result<Extraction, ExtractError>;

pub enum EventKind { PriceDrop, PriceRise, TargetHit, LowestEver, BackInStock, OutOfStock, InflatedSale }
pub struct Event { pub watch_id: i64, pub kind: EventKind, pub old: Option<Money>, pub new: Option<Money> }
pub fn evaluate(watch: &Watch, history: &[Observation], latest: &Observation) -> Vec<Event>;

pub async fn fetch(client: &Fetcher, url: &Url, previous: Option<&Validators>) -> Result<Fetched, FetchError>;
pub enum Fetched { Body { html: String, validators: Validators }, NotModified }
```

Database tables:

- `watches`: id, url (unique), name, selector, target_minor, currency, interval_s, enabled, status (`ok`, `needs-selector`, `blocked`, `error`), etag, last_modified, next_check_at, failures, created_at.
- `observations`: id, watch_id, at, price_minor, list_price_minor, currency, availability, source. One row per successful check.
- `events`: id, watch_id, at, kind, old_minor, new_minor, notified_at.
- `schema_version`: the applied migration number.

### Data flow

Scheduler tick → due watches (ordered by `next_check_at`) → per-host limiter → `fetch` → `extract` → insert observation → `rules::evaluate` against history → insert events → `notify` for events with no `notified_at` → set `next_check_at` (interval ± jitter, or backoff). The web UI and CLI only read the database, plus add, edit, pause and delete watches.

### Errors

- One `Error` enum (thiserror) per layer: `ExtractError` (`NoPrice`, `AmbiguousPrice`, `BadSelector`), `FetchError` (`Blocked` for robots.txt or 403, `RateLimited`, `Http(status)`, `Network`, `TooLarge`), `DbError`, `ConfigError`, `NotifyError`.
- A failed check never stops the scheduler. It is recorded on the watch (`status`, `failures`) and the next check is pushed back.
- A failed notification leaves `notified_at` empty, so it is retried on the next tick.
- `main` prints `ConfigError` to stderr and exits 2; any other error exits 1.
- Logging: `tracing` to stderr, level from `--log-level`; journald collects it under systemd.

### Configuration

Environment variables, read once at startup:

- `HAGGLE_DB`: database path. Required. `/mnt/database/haggle/haggle.db` on hp-server.
- `HAGGLE_LISTEN`: listen address, default `127.0.0.1:8421`; `192.168.5.21:8421` on hp-server.
- `HAGGLE_NTFY_URL`, `HAGGLE_NTFY_TOKEN`: notification topic and optional token. The token is a secret and lives only in the systemd environment file.

```
haggle serve                     run the scheduler and the web UI
haggle add URL [--name N] [--target 199.99] [--selector CSS] [--every 6h]
haggle ls                        watches with latest price and status
haggle check URL|ID              fetch once and print what was found, without storing it
haggle history ID                price history as a table
haggle rm ID
haggle demo                      fake shop + temp database + UI, with time sped up
```

### Dependencies

Runtime: tokio, axum, reqwest (rustls), scraper, rusqlite (bundled), askama, clap, serde, serde_json, url, thiserror, tracing, tracing-subscriber, time
Dev: none beyond the standard test harness; `cargo clippy` and `cargo fmt` from the toolchain

### Design decisions

Each line: what was chosen · the alternative · why.

- Structured data before selectors · selectors only · JSON-LD survives a page redesign; a selector does not.
- No headless browser · bundling Chromium · it would be most of the binary and most of the RAM on an 8 GB server for a minority of pages.
- SQLite · Postgres · one user, a few hundred watches, one writer; a file on the database volume is simpler to back up and restore.
- Every check stored · only changes stored · the chart and the "real discount" rule need the full series, and a year of 6-hourly checks for 200 watches is under 300k rows.
- `extract` and `rules` pure · mixed with I/O · they are the logic most likely to be wrong, and pure functions are tested with fixtures and no network.
- Server-rendered HTML and SVG · a React front end · the UI is five pages; a second toolchain would outweigh it.
- Discount measured against observed history · trusting the shop's "was" price · the shop's reference price is the thing being checked.
- Build on hp-server · a static musl build on the workstation, copied with `scp` · one machine to build, test and run on; the binary always matches the server's glibc, and no musl C toolchain is needed. The cost is a Rust toolchain on the server and slower first builds on the i5-8500T.

## Conventions

Workflow: solo
Guardrails: set (squash merges only, merged branches deleted, the base branch reached only through pull requests)
Format: `cargo fmt --all --check`
Lint: `cargo clippy --all-targets -- -D warnings`
CI: off (the board's full check is the gate)

- Build: `make build` (`cargo build --release`).
- Test: `make test` (`cargo test`). A single test: `cargo test <name>`.
- Full check: `make check` (format check, lint, build, tests).
- Demo: `make demo` (`haggle demo`).
- Install: `make install`, run on hp-server (copies `target/release/haggle` to `/usr/local/bin/` and the unit files to `/etc/systemd/system/`, runs `systemctl daemon-reload`; uses sudo; does not restart anything by itself).
- Update: `git pull && make check && make install`, then `sudo systemctl restart haggle`.
- Tests: extraction is tested against saved pages in `tests/fixtures/`, each with its expected price written by hand, never taken from the extractor's own output. Fetch tests run against a local test server. No test touches the internet or the live database.
- Naming: standard Rust (snake_case functions, PascalCase types). SQL identifiers are snake_case.
- Comments: a `//!` header on every module, `///` on every public item, inline comments only for the why.
- Per milestone: branch `board/m<k>-<name>`, one commit per card, and a pull request once `make check` is green.

## Milestones

### M1 · Read a price from a page

Goal: `haggle check URL` prints the price, currency, availability and where it found them.
Depends on: none
- [x] Cargo project, `Makefile` targets, `rustfmt.toml`, `.gitignore`, in `Cargo.toml`, `Makefile` (#1)
- [x] Error enums and exit codes, in `src/main.rs` and each module (#2)
- [x] `Money`, currency codes and price-string parsing, in `src/money.rs` (#3)
- [x] JSON-LD extractor (`Product`, `Offer`, `AggregateOffer`, `@graph`), in `src/extract/jsonld.rs` (#4)
- [x] Meta-tag extractor, in `src/extract/meta.rs` (#5)
- [x] Microdata extractor, in `src/extract/microdata.rs` (#6)
- [x] CSS-selector extractor, in `src/extract/selector.rs` (#7)
- [x] The ordered extraction chain, in `src/extract/mod.rs` (#8)
- [x] HTTP fetch with size limit, timeout and User-Agent, in `src/fetch.rs` (#9)
- [x] `check` subcommand, in `src/cli.rs` (#10)
- [x] Test: price strings in US, European and bare formats parse to the right minor units, in `src/money.rs` (#11)
- [x] Test: each extractor against at least three saved pages, in `tests/extract.rs`, `tests/fixtures/` (#12)
- [x] Test: a page with no price returns `NoPrice`, not a guess, in `tests/extract.rs` (#13)

Done when: `make check` passes and `haggle check` on each fixture served locally prints the expected price.
Note: The price-parsing tests live in tests/money.rs instead of src/money.rs, so a tester writes them. Extractor tests are split per extractor (tests/extract_*.rs), and tests/extract.rs covers the chain. `check` takes a URL only; IDs come with the database in M2.

### M2 · Watch, remember, notify

Goal: watches are checked on a schedule and a price drop sends one notification.
Depends on: M1
- [ ] Database open, pragmas (WAL, foreign keys) and embedded migrations, in `src/db/mod.rs`, `migrations/`
- [ ] Watches, observations and events queries, in `src/db/`
- [ ] Guard that refuses a `/mnt/database` path in tests and the demo, in `src/config.rs`
- [ ] robots.txt cache, per-host limiter, conditional GET and backoff, in `src/fetch.rs`
- [ ] Event rules: drop, rise, target hit, lowest ever, stock changes, inflated sale, in `src/rules.rs`
- [ ] ntfy client, in `src/notify.rs`
- [ ] Scheduler loop with jitter and graceful shutdown, in `src/scheduler.rs`
- [ ] `add`, `ls`, `history`, `rm` and `serve` subcommands, in `src/cli.rs`
- [ ] Test: each rule against a hand-written history, in `src/rules.rs`
- [ ] Test: the same event is never notified twice, including after a restart, in `tests/scheduler.rs`
- [ ] Test: a 429 response pushes the next check back and stops requests to that host, in `tests/fetch.rs`
- [ ] Test: a robots.txt disallow marks the watch `blocked` and sends no request, in `tests/fetch.rs`

Done when: `make check` passes and, against a local test shop, a scripted price drop produces exactly one ntfy request.

### M3 · Dashboard and demo

Goal: the web UI shows every watch with its chart, and anyone can see it work in a minute.
Depends on: M2
- [ ] Router, shared layout and stylesheet, in `src/web/mod.rs`, `templates/`, `static/`
- [ ] Dashboard: watches with latest price, change, status and a sparkline, in `src/web/pages.rs`
- [ ] Watch detail: full chart, event list, edit target and interval, pause, delete, in `src/web/pages.rs`
- [ ] Add-watch form with a live preview of what was extracted, in `src/web/pages.rs`
- [ ] SVG price chart with target line and stock gaps, in `src/web/chart.rs`
- [ ] Fake shop with a scripted price and stock timeline, in `src/demo_shop.rs`
- [ ] `demo` subcommand: fake shop, temp database, sped-up clock, in `src/cli.rs`
- [ ] Test: chart output for a known series, in `src/web/chart.rs`
- [ ] Test: each page renders for an empty database and for a populated one, in `tests/web.rs`

Done when: `make demo` opens a dashboard where a price drop and a restock appear within one minute.

### M4 · Running on hp-server

Goal: Haggle watches real items from the homelab.
Depends on: M3
- [ ] systemd unit with `ReadWritePaths=/mnt/database/haggle` and an environment file, in `deploy/haggle.service`
- [ ] Daily backup (`VACUUM INTO`, seven kept) to `/mnt/database/haggle/backups/`, in `deploy/haggle-backup.service`, `deploy/haggle-backup.timer`
- [ ] `make install` target, in `Makefile`
- [ ] Install and update guide, including the toolchain setup (apt packages, rustup) and the one-time folder creation, in `deploy/README.md`
- [ ] Extraction and scheduling design notes, in `docs/design/`
- [ ] README screenshots and demo GIF, in `media/`

Done when: the service has run on hp-server for 48 hours, has stored observations for at least five real items, and a restore from a backup file has been tried once.

## Out of scope

- JavaScript-rendered pages and headless browsers.
- Logging in to shops, carts, checkout or any purchase.
- Multiple users and accounts.
- Searching for a product across shops; a watch is one URL.
- Browser extension and mobile app.
- Access from outside the LAN.

## Risks and open questions

- Many large shops block non-browser clients or hide prices behind JavaScript. Haggle will not work for those; the README must say which kinds of shop work and show the `needs-selector` and `blocked` states plainly.
- A shop's terms may forbid automated access. The polite defaults lower the load but do not change the terms; the README says so.
- Building on an 8 GB, 6-core server: the first release build (tokio, axum, reqwest, bundled SQLite) takes several minutes and uses a few GB of RAM while other services run. If memory gets tight, limit parallel jobs with `cargo build -j 2`.
- `/mnt/database` is owned by root, so creating `/mnt/database/haggle` needs sudo once, by the user.
- Whether ntfy is self-hosted or the public ntfy.sh is not decided; the code only needs a URL.
