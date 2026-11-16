# Haggle

A self-hosted price and stock watcher: give it a product URL, and it tells you when the price drops or the item is back in stock.

> **Status: planned.** Nothing is built yet. This README describes the plan in [docs/plan/PLAN.md](docs/plan/PLAN.md). Every feature and command below is a target, not a working result.

## Overview

Haggle checks product pages on a schedule, keeps the price and availability history of each one, and sends a notification when something changes that you care about. A small web dashboard lists every watched item with its price chart.

It is built to be used: it will run on a home server and watch real items before they are bought.

What it demonstrates:

- reading a price from a web page reliably: structured data first (schema.org JSON-LD, meta tags, microdata), a CSS selector only as the last resort,
- being a polite client: robots.txt, per-host rate limits, conditional requests, backoff,
- money handled as integer minor units with a currency, never floating point,
- a discount judged against the price history Haggle observed, not the shop's own "was" price,
- one static Rust binary with an embedded SQLite database and a server-rendered UI.

## Features

All features are planned.

- Watch any product URL; set a target price and a check interval per item.
- Notifications through ntfy for: price drop, target reached, lowest price seen, out of stock, back in stock, and a "sale" that is not one.
- Price history chart per item, drawn as SVG on the server.
- Clear states for pages it cannot read (`needs-selector`, `blocked`), instead of silent failure.
- A demo mode with a built-in fake shop, so the whole loop can be seen in a minute.

## Requirements

- Rust (stable), to build.
- Linux x86-64, to run. The release build is a static musl binary.
- An ntfy topic, for notifications (optional).

## Quick start

These commands are planned.

```sh
make demo                      # fake shop + temporary database + dashboard
haggle add https://shop.example/item --target 199.99
haggle serve
```

## Where the data lives

One SQLite file. On the home server it is `/mnt/database/haggle/haggle.db`. Tests and the demo use temporary files and refuse to open that path.

## Limits

Haggle does not run JavaScript, so shops that render prices in the browser, or that block non-browser clients, will not work. It never logs in and never buys anything.

## Plan

See [docs/plan/PLAN.md](docs/plan/PLAN.md) for the decisions, architecture and milestones.

## License

MIT (planned).
