# Haggle

Know when a product's price drops or the item is back in stock.

![Rust edition 2024](https://img.shields.io/badge/rust-edition%202024-orange)

## Overview

Haggle is a self-hosted price and stock watcher: give it a product URL, and it tells you when the price drops or the item is back in stock. It is for personal use on a home server, for buying homelab, hobby and work items at their lowest price. Haggle will judge a discount against the price history it observed itself, not against the shop's own "was" price. It does not run JavaScript and never logs in or buys anything, so shops that render prices in the browser, or block non-browser clients, will not work.

## Features

- `haggle check URL` reads a product's price from its page, with the currency, list price, availability and title when the page gives them.
- Prices are tried in order: schema.org JSON-LD, Open Graph and `product:price:*` meta tags, schema.org microdata, then a CSS selector you supply with `--selector`. The first valid price wins, and the output names its source.
- Money is integer minor units with an ISO 4217 currency code, never floating point.
- Fetches are bounded and identify the client: a 20-second timeout, a 5 MiB size limit, and a User-Agent that names the project and its repository.
- Watch any product URL; set a target price and a check interval per item (planned).
- Notifications through ntfy for: price drop, target reached, lowest price seen, out of stock, back in stock, and a "sale" that is not one (planned).
- Price history chart per item, drawn as SVG on the server (planned).
- Clear states for pages it cannot read (`needs-selector`, `blocked`), instead of silent failure (planned).
- A demo mode with a built-in fake shop, so the whole loop can be seen in a minute (planned).

## Quick start

Prerequisites:

- Rust stable, installed with [rustup](https://rustup.rs). The project uses edition 2024, which Ubuntu's apt `rustc` is too old for.
- A C compiler and git. On Ubuntu, install `build-essential` and `git` from apt.
- Linux on x86-64, the target platform.

Clone and build:

```sh
git clone https://github.com/NC4321/haggle.git
cd haggle
make build
```

Check a product page. Use any product page URL; the command fetches the page once and prints what it found, without storing anything:

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

## Usage

`haggle check` takes one product page URL (http or https) and an optional selector. It stores nothing: it writes no database and no files.

For a page without structured data, give the CSS selector of the price element. The source line then reads `selector`:

```sh
./target/release/haggle check https://shop.example/item --selector '.price'
```

```text
price: 1299.00 EUR
availability: unknown
title: Example Phone
source: selector
```

If no price can be found, the command does not guess. It exits with status 1 and reports the problem on stderr:

```sh
./target/release/haggle check https://shop.example/no-price
```

```text
error: no price found on the page
```

## Configuration

The only setting built so far is a global flag. The rest is planned and read from environment variables at startup.

| Setting | Default | What it does |
| --- | --- | --- |
| `--log-level LEVEL` (flag, all commands) | `warn` | Log filter written to stderr, for example `debug` or `haggle=trace`. |
| `HAGGLE_DB` (planned, required) | none | Path of the SQLite database. On the home server it is `/mnt/database/haggle/haggle.db`. Tests and the demo use temporary files and refuse to open that path. |
| `HAGGLE_LISTEN` (planned) | `127.0.0.1:8421` | Address the web UI listens on. |
| `HAGGLE_NTFY_URL`, `HAGGLE_NTFY_TOKEN` (planned) | none | ntfy topic URL for notifications, and an optional bearer token. The token is a secret. |

## Project structure

```text
haggle/
├── Cargo.toml            # package, binary and dependencies
├── Makefile              # build, test, check, demo and install targets
├── rustfmt.toml          # formatter settings
├── docs/plan/PLAN.md     # decisions, architecture and milestones
├── src/
│   ├── main.rs           # the haggle binary: arguments, logging, exit status
│   ├── lib.rs            # the library crate and its modules
│   ├── cli.rs            # the check command and its arguments and errors
│   ├── fetch.rs          # page fetching with timeout, size limit and User-Agent
│   ├── money.rs          # Money: integer minor units with a currency code
│   └── extract/
│       ├── mod.rs        # the ordered extraction chain and the result types
│       ├── jsonld.rs     # schema.org JSON-LD extractor
│       ├── meta.rs       # Open Graph and product meta tag extractor
│       ├── microdata.rs  # schema.org microdata extractor
│       └── selector.rs   # CSS selector extractor
└── tests/
    ├── fixtures/             # saved HTML pages with hand-written expected prices
    ├── cli_check.rs          # the check command, run as the built binary
    ├── extract.rs            # the ordered chain over the saved pages
    ├── extract_jsonld.rs     # JSON-LD extractor tests
    ├── extract_meta.rs       # meta tag extractor tests
    ├── extract_microdata.rs  # microdata extractor tests
    ├── extract_selector.rs   # CSS selector extractor tests
    ├── fetch.rs              # fetch tests against a local test server
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

Extraction is tested against saved pages in `tests/fixtures/`, grouped by extractor (`jsonld`, `meta`, `microdata`, `selector`), with `chain` for the ordered chain and `cli` for `haggle check`. Each page's expected price is written by hand, never copied from the extractor's output. Fetch tests run against a local test server. No test touches the internet or a live database.

## Roadmap

- [ ] **M1 · Read a price from a page.** `haggle check URL` prints the price, currency, availability and where it found them.
- [ ] **M2 · Watch, remember, notify.** Watches are checked on a schedule, and a price drop sends one notification.
- [ ] **M3 · Dashboard and demo.** The web UI shows every watch with its chart, and anyone can see it work in a minute.
- [ ] **M4 · Running on hp-server.** Haggle watches real items from the homelab.

The decisions, architecture and task list of each milestone are in [docs/plan/PLAN.md](docs/plan/PLAN.md).

## Contributing

This repository is built milestone by milestone with agent-board. Each milestone has one branch, `board/m<k>-<name>` (for example `board/m1-read-a-price`), with one commit per card, and one pull request into `main`. Humans review and merge. Merges are squash merges, and merged branches are deleted.

Before you open a pull request, run the checks. There is no CI yet, so `make check` is the gate:

```sh
make check
```

## License

No license yet.
