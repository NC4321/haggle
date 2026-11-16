-- Initial schema: watched product pages, their price observations and the events they raise.

CREATE TABLE watches (
    id INTEGER PRIMARY KEY,
    url TEXT NOT NULL UNIQUE,
    name TEXT,
    selector TEXT,
    target_minor INTEGER,
    currency TEXT,
    interval_s INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'ok',
    etag TEXT,
    last_modified TEXT,
    next_check_at INTEGER NOT NULL,
    failures INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE observations (
    id INTEGER PRIMARY KEY,
    watch_id INTEGER NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    price_minor INTEGER NOT NULL,
    list_price_minor INTEGER,
    currency TEXT NOT NULL,
    availability TEXT NOT NULL,
    source TEXT NOT NULL
) STRICT;

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    watch_id INTEGER NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    old_minor INTEGER,
    new_minor INTEGER,
    currency TEXT,
    notified_at INTEGER
) STRICT;

CREATE INDEX observations_watch_id_at ON observations (watch_id, at);
CREATE INDEX events_notified_at ON events (notified_at);
CREATE INDEX watches_next_check_at ON watches (next_check_at);
