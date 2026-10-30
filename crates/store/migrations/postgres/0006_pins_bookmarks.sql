-- Pinned messages (pins.*). Unpinned automatically when the message goes.
CREATE TABLE pins (
    channel_id  TEXT COLLATE "C" NOT NULL,
    ts          TEXT COLLATE "C" NOT NULL,
    pinned_by   TEXT COLLATE "C" NOT NULL,
    created     BIGINT NOT NULL,
    PRIMARY KEY (channel_id, ts),
    FOREIGN KEY (channel_id, ts) REFERENCES messages(channel_id, ts) ON DELETE CASCADE
);

-- Channel bookmarks (bookmarks.*), shown in order of creation.
CREATE TABLE bookmarks (
    id          TEXT COLLATE "C" PRIMARY KEY,
    channel_id  TEXT COLLATE "C" NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    link        TEXT NOT NULL,
    emoji       TEXT,
    app_id      TEXT COLLATE "C",
    created     BIGINT NOT NULL,
    updated     BIGINT NOT NULL,
    updated_by  TEXT COLLATE "C" NOT NULL
);
CREATE INDEX bookmarks_channel ON bookmarks(channel_id, created);
