# Haggle

Know when a product's price drops or the item is back in stock.

![Rust edition 2024](https://img.shields.io/badge/rust-edition%202024-orange)

## Overview

Haggle is a self-hosted price and stock watcher: give it a product URL, and it tells you when the price drops or the item is back in stock. It is for personal use on a home server, for buying homelab, hobby and work items at their lowest price. It judges a discount against the price history it observed itself, not against the shop's own "was" price. It does not run JavaScript and never logs in or buys anything, so shops that render prices in the browser or block non-browser clients will not work.

## Features

- `haggle check URL` reads a product's price from its page, with the currency, list price, availability and title when the page gives them.
- Prices are tried in order: schema.org JSON-LD, Open Graph and `product:price:*` meta tags, schema.org microdata, then a CSS selector you supply with `--selector`. The first valid price wins, and the output names its source.
- Money is integer minor units with an ISO 4217 currency code, never floating point.
- Fetches are bounded and identify the client: a 20-second timeout, a 5 MiB size limit, and a User-Agent that names the project and its repository.
- Watches are stored in SQLite. Add a product with an optional target price and check interval, list the watches, read their price history and remove them.
- `haggle serve` checks due watches every 30 seconds, honours robots.txt and backs off after HTTP 429 responses.
- Clear states for pages it cannot read: a watch is marked `needs-selector` or `blocked`, and `haggle ls` shows the state.
- ntfy notifications, one message per check, for price drops and rises, a target reached, the lowest price seen, out of stock and back in stock, and a "sale" that is not one.
- A web dashboard with every watch and its price chart, drawn as SVG on the server (planned).
- A demo mode with a built-in fake shop, so the whole loop can be seen in a minute (planned).

## Quick start

Prerequisites:

- Rust stable, edition 2024, installed with [rustup](https://rustup.rs). Ubuntu's apt `rustc` is too old for edition 2024.
- A C compiler and git. On Ubuntu, install `build-essential` and `git` from apt.
- Linux on x86-64, the target platform.

Clone and build:

```sh
git clone https://github.com/NC4321/haggle.git
cd haggle
make build
```

Check a product page. Use any product page URL. The command fetches the page once and prints what it found, without storing anything:

```sh
./target/release/haggle check https://shop.example/item
```

For a page with schema.org data, it prints something like this:

```text
price: 1299.00 EUR
availability: in stock
title: Example Phone
source: json-ld
```

If no price can be found, the command does not guess. It exits with status 1 and prints `error: no price found on the page` to stderr.

## Usage

Commands other than `check` store their data in the database named by `HAGGLE_DB`, described under [Configuration](#configuration). The examples below set it inline.

Watch a product with a target price. `--target` needs the currency, `--every` defaults to `6h` and must be at least `5m`:

```sh
HAGGLE_DB=./haggle.db ./target/release/haggle add https://shop.example/item --name "Example Phone" --target "199.99 EUR" --every 6h
```

```text
added watch 1
```

List the watches. Each line shows the id, the status, the latest price (`-` until the first check) and the name, or the URL when no name was given:

```sh
HAGGLE_DB=./haggle.db ./target/release/haggle ls
```

```text
1  ok  -  Example Phone
```

Show a watch's recorded prices, oldest first. `rm ID` stops the watch and deletes its history.

```sh
HAGGLE_DB=./haggle.db ./target/release/haggle history 1
```

```text
2026-11-16 18:00 UTC  1299.00 EUR  in stock
```

Run the scheduler. It checks due watches every 30 seconds and runs until Ctrl-C or SIGTERM. Each check sends at most one ntfy message per watch, covering the events it found. Without `HAGGLE_NTFY_URL`, it still checks watches and logs that notifications are off:

```sh
HAGGLE_DB=./haggle.db HAGGLE_NTFY_URL=https://ntfy.example/haggle ./target/release/haggle serve
```

## Configuration

Environment variables are read once at startup. `check` needs none of them.

| Setting | Default | What it does |
| --- | --- | --- |
| `--log-level LEVEL` (flag, all commands) | `warn` | Log filter written to stderr, for example `debug` or `haggle=trace`. |
| `HAGGLE_DB` (required by `add`, `ls`, `history`, `rm`, `serve`) | none | Path of the SQLite database file, created if it does not exist. If unset, the command fails with exit status 2. On the planned home server it is `/mnt/database/haggle/haggle.db`; tests and the planned demo use temporary files and never open that live path. |
| `HAGGLE_NTFY_URL` (optional) | none | ntfy topic URL for notifications. Without it, `serve` runs with notifications off. |
| `HAGGLE_NTFY_TOKEN` (optional) | none | Bearer token for the ntfy topic. It is ignored unless `HAGGLE_NTFY_URL` is set. It is a secret. |
| `HAGGLE_LISTEN` (planned, with the web UI) | `127.0.0.1:8421` | Address the web UI listens on. It is already validated at startup. |

Configuration errors, such as an unset `HAGGLE_DB` or a malformed `HAGGLE_LISTEN`, exit with status 2. Other errors exit with status 1.

## Project structure

```text
haggle/
├── Cargo.toml                # package, binary and dependencies
├── Makefile                  # build, test and check targets
├── rustfmt.toml              # formatter settings
├── migrations/
│   └── 001_init.sql          # the first schema, applied when the database opens
├── docs/plan/PLAN.md         # decisions, architecture and milestones
├── src/
│   ├── main.rs               # the haggle binary: arguments, logging, exit status
│   ├── lib.rs                # the library crate and its modules
│   ├── cli.rs                # the check, add, ls, history, rm and serve commands
│   ├── config.rs             # environment variables into one validated Config
│   ├── fetch.rs              # page fetching: timeout, size limit, robots.txt, 429 pauses
│   ├── scheduler.rs          # the check loop: due watches, storage and notifications
│   ├── rules.rs              # event rules over a watch's price history
│   ├── notify.rs             # ntfy client
│   ├── money.rs              # Money: integer minor units with a currency code
│   ├── db/
│   │   ├── mod.rs            # opening the database and applying migrations
│   │   ├── watches.rs        # watch queries
│   │   ├── observations.rs   # observation queries
│   │   └── events.rs         # event queries
│   └── extract/
│       ├── mod.rs            # the ordered extraction chain and the result types
│       ├── jsonld.rs         # schema.org JSON-LD extractor
│       ├── meta.rs           # Open Graph and product meta tag extractor
│       ├── microdata.rs      # schema.org microdata extractor
│       └── selector.rs       # CSS selector extractor
└── tests/
    ├── fixtures/             # saved HTML pages with hand-written expected prices
    ├── cli_check.rs          # the check command
    ├── cli_watches.rs        # the add, ls, history and rm commands
    ├── cli_serve.rs          # the serve command and its exit codes
    ├── config.rs             # configuration parsing and the live database guard
    ├── db.rs                 # opening the database and migrations
    ├── db_watches.rs         # watch queries
    ├── db_observations.rs    # observation queries
    ├── db_events.rs          # event queries
    ├── extract.rs            # the ordered chain over the saved pages
    ├── extract_jsonld.rs     # JSON-LD extractor tests
    ├── extract_meta.rs       # meta tag extractor tests
    ├── extract_microdata.rs  # microdata extractor tests
    ├── extract_selector.rs   # CSS selector extractor tests
    ├── fetch.rs              # fetch tests against a local test server
    ├── robots.rs             # robots.txt and rate limit tests
    ├── rules.rs              # event rule tests
    ├── scheduler.rs          # scheduler ticks and notification tests
    ├── notify.rs             # ntfy client tests
    └── money.rs              # price string parsing tests
```

## Development

Run these from the project root:

```sh
make build                                   # release build (cargo build --release)
make test                                    # all tests (cargo test)
cargo test <name>                            # one test, by name
make check                                   # format check, lint, release build and tests
cargo fmt --all --check                      # format check only
cargo clippy --all-targets -- -D warnings    # lint only
```

Integration tests live in `tests/`, one file per module, and command tests are named `cli_*`. Extraction is tested against saved pages in `tests/fixtures/`, grouped by extractor (`jsonld`, `meta`, `microdata`, `selector`), with `chain` for the ordered chain and `cli` for `haggle check`. Each page's expected price is written by hand, never copied from the extractor's output. Fetch tests run against a local test server. No test touches the internet or a live database.

## Roadmap

- [x] **M1 · Read a price from a page.** `haggle check URL` prints the price, currency, availability and where it found them.
- [ ] **M2 · Watch, remember, notify.** Watches are checked on a schedule, and a price drop sends one notification.
- [ ] **M3 · Dashboard and demo.** The web UI shows every watch with its chart, and anyone can see it work in a minute.
- [ ] **M4 · Running on hp-server.** Haggle watches real items from the homelab.

The decisions, architecture and task list of each milestone are in [docs/plan/PLAN.md](docs/plan/PLAN.md).

## Contributing

This repository is built milestone by milestone with agent-board. Each milestone has one branch, `board/m<k>-<name>` (for example `board/m2-watch-remember-notify`), with one commit per card, and one pull request into `main`. Humans review and merge. Merges are squash merges, and merged branches are deleted.

Before you open a pull request, run the checks. There is no CI yet, so `make check` is the gate:

```sh
make check
```

## License

No license yet.
