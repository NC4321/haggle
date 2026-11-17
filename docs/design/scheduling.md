# Scheduling and checking

How haggle decides when and how to check a watch, and what happens afterwards.
Sources: `src/scheduler.rs`, `src/fetch.rs`, `src/rules.rs`, `src/notify.rs`, `src/db/`
and the Architecture section of `docs/plan/PLAN.md`.

## The tick

`scheduler::run` calls `scheduler::tick` every `TICK` (30 s) until shutdown; the
check in progress finishes first. One tick:

1. `watches::due(conn, now, BATCH)` returns up to `BATCH` (50) watches.
2. Each is checked one after another (`check_watch`).
3. If a `Notifier` is configured, `notify_pending` sends pending events.

A failing watch is recorded and the tick goes on. Only a `DbError` stops the tick,
and `run` then returns it. `TickReport` counts `checked`, `failed`, `events`, `notified`.

## Picking due watches

`watches::due` (`src/db/watches.rs`) selects rows with `enabled = 1 AND next_check_at <= now`,
ordered by `next_check_at, id`. `next_check_at` is unix seconds, set when the watch is
inserted and rewritten after every check (success or failure).

## Interval and jitter

Each watch has `interval_s`. After a success the next check is
`now + jittered(interval_s, random_unit(id, now))`. `jittered` moves the interval by
up to `JITTER_FRACTION` (0.1, so 10 %), rounds, and never returns less than 1.
`random_unit` hashes `(watch_id, now)` with a per-process `RandomState`.
The CLI default interval is `6h` and its minimum is `MIN_INTERVAL_SECONDS` (300 s), in `src/cli.rs`.

## Per-host limiter

`fetch::Fetcher` keeps a `HostState` per `host:port` key (`last_request`, `paused_until`).
`Politeness::default()`:

| Field | Value | Meaning |
|---|---|---|
| `host_interval` | 5 s | minimum gap between requests to one host |
| `robots_ttl` | 24 h | how long a robots.txt is reused |
| `rate_limit_pause` | 1 h | minimum pause after a 429 |

`wait_for_turn` reserves the request slot before sleeping, so concurrent callers
queue one `host_interval` apart. The robots.txt request also takes a turn.

## robots.txt and its cache

`fetch_page` order: host key, paused check, robots check, `wait_for_turn`, request.

- `robots_allow` uses the cached body while younger than `robots_ttl`, else `refresh_robots`.
- The cache is keyed by origin. Reading stops at `MAX_ROBOTS_BYTES` (500 KiB).
- 2xx: the body is parsed and cached. Other 4xx: empty body, allow all, cached.
  5xx: `FetchError::Blocked`, not cached. A failed request is `FetchError::Network`.
- Matching uses the agent token `ROBOTS_AGENT` (`haggle`), falling back to `*`;
  the longest matching pattern wins, and Allow wins a tie (`robots_allows`).
- A disallowed URL gives `FetchError::Blocked`.

## Conditional GET

`check_watch` sends the watch's stored `etag` and `last_modified` as `Validators`
(`If-None-Match`, `If-Modified-Since`). Limits: `DEFAULT_TIMEOUT` 20 s,
`DEFAULT_MAX_BYTES` 5 MiB, `CONNECT_TIMEOUT` 10 s, `MAX_REDIRECTS` 5.

- `Fetched::Body { html, validators }`: the page is extracted (`extract::extract`
  with the watch's `selector`) and stored as a new observation.
- `Fetched::NotModified` (304): the latest observation is copied with `at = now`
  (`repeat_at`) and goes through the rules like any other. With no history nothing is recorded.

Either way `record_success` stores the validators returned.

## Failures and backoff

A page failure (`PageFailure`: bad URL, `FetchError`, `ExtractError`) calls
`record_failure`, which does `failures = failures + 1` and stores the status and next check.

| Status | Set when |
|---|---|
| `ok` | a check succeeded; `failures` resets to 0 |
| `needs-selector` | `NoPrice` or `AmbiguousPrice` and the watch has no `selector` |
| `blocked` | `FetchError::Blocked` (robots.txt, its 5xx, or HTTP 403) |
| `error` | anything else, including `RateLimited` and `Http(_)` |

The retry time is `now + backoff_delay(interval, failures + 1)`: the interval doubled
once per failure, capped at `MAX_BACKOFF` (7 days), never below the interval. No jitter is
applied. After `FetchError::RateLimited` it is the later of that and the end of the host
pause (`paused_until`). A 429 pauses the host for the longer of `rate_limit_pause` and a
numeric `Retry-After` (the HTTP-date form is ignored). While paused, `fetch_page`
returns `RateLimited` without any request.

## What is stored per check

- `observations` (`src/db/observations.rs`): `watch_id`, `at`, `price_minor`,
  `list_price_minor`, `currency`, `availability`, `source`. Index on `(watch_id, at)`.
- `events`: `watch_id`, `at`, `kind`, `old_minor`, `new_minor`, `currency`, `notified_at`.
- `watches`: `status`, `failures`, `etag`, `last_modified`, `next_check_at`.

`record_observation` loads `history`, inserts the new row, calls `rules::evaluate`,
and inserts each returned event with `at = now`. Failed checks store no observation.

## Rules

`rules::evaluate(watch, history, latest)` is pure. `history` is every earlier
observation, oldest first. Comparisons only use observations in the currency of `latest.price`.

| `EventKind` | Fires when |
|---|---|
| `PriceDrop` | price is below the previous observation's |
| `PriceRise` | price is above the previous observation's |
| `TargetHit` | price `<=` the watch `target` and the previous price was above it (or there is none) |
| `LowestEver` | price is below every earlier price |
| `BackInStock` | previous was `OutOfStock` and latest is `InStock` or `Preorder` |
| `OutOfStock` | previous was `InStock` or `Preorder` and latest is `OutOfStock` |
| `InflatedSale` | see below |

Events come out in `EventKind` declaration order.

### InflatedSale

The shop's "was" price is `latest.list_price`. It fires when all of these hold:

- `list_price` is in the same currency and above the current price.
- At least `SALE_MIN_HISTORY` (3) earlier observations lie within
  `SALE_LOOKBACK_S` (90 days) before `latest.at`.
- The highest observed price in that window is below `list_price`: the "was" price was never charged.
- It is a new claim: the previous observation had a different `list_price`
  (or there is no previous), so a steady claim does not fire on every check.

## Notification retry

`notify_pending` reads `events::unnotified` (`notified_at IS NULL`, by id) and groups
them by `(watch_id, at)`: one message per watch per check (`notify::message_for`).
The lead event is the first in `LEAD_ORDER` (`TargetHit`, `LowestEver`, `InflatedSale`,
`PriceDrop`, `BackInStock`, `OutOfStock`, `PriceRise`); a `TargetHit` makes the message high priority.
`NOTIFY_TIMEOUT` is 10 s.

- Sent: `mark_notified` sets `notified_at = now` on the group's events.
- Failed: a warning is logged, `notified_at` stays empty, and the group is retried
  on the next tick. There is no retry limit or delay.
- A group whose watch no longer exists is skipped.

## Sharing the database

`db::open` sets `journal_mode = WAL`, `foreign_keys = ON` and `busy_timeout` of
`BUSY_TIMEOUT_MS` (5000). The scheduler and the web UI each open their own
`Connection` to the same file; WAL lets the web UI read while the scheduler
writes, and the busy timeout covers writer contention. The web side wraps its
connection in `Arc<Mutex<_>>` (`AppState`, PLAN.md).
