-- Pinned messages (pins.*). Unpinned automatically when the message goes.
CREATE TABLE pins (
    channel_id  TEXT NOT NULL,
    ts          TEXT NOT NULL,
    pinned_by   TEXT NOT NULL,
    created     INTEGER NOT NULL,
    PRIMARY KEY (channel_id, ts),
    FOREIGN KEY (channel_id, ts) REFERENCES messages(channel_id, ts) ON DELETE CASCADE
);

-- Channel bookmarks (bookmarks.*), shown in order of creation.
CREATE TABLE bookmarks (
    id          TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    link        TEXT NOT NULL,
    emoji       TEXT,
    app_id      TEXT,
    created     INTEGER NOT NULL,
    updated     INTEGER NOT NULL,
    updated_by  TEXT NOT NULL
);
CREATE INDEX bookmarks_channel ON bookmarks(channel_id, created);
