-- Files (files.getUploadURLExternal / completeUploadExternal). Contents live
-- in their own table so listing files never reads the bytes.
CREATE TABLE files (
    id            TEXT PRIMARY KEY,
    team_id       TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id       TEXT NOT NULL,
    bot_id        TEXT,
    name          TEXT NOT NULL,
    title         TEXT NOT NULL,
    mimetype      TEXT NOT NULL,
    filetype      TEXT NOT NULL,
    -- Set for text snippets (`snippet_type`).
    snippet_type  TEXT,
    alt_txt       TEXT,
    size          INTEGER NOT NULL,
    created       INTEGER NOT NULL,
    -- Secret part of the upload URL, until the bytes arrive.
    upload_token  TEXT UNIQUE,
    uploaded      INTEGER NOT NULL DEFAULT 0,
    -- completeUploadExternal has been called.
    completed     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX files_team ON files(team_id, created);

CREATE TABLE file_contents (
    file_id  TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    data     BLOB NOT NULL
);

-- Where a file was shared (one row per message it was posted with).
CREATE TABLE file_shares (
    file_id     TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    channel_id  TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ts          TEXT NOT NULL,
    PRIMARY KEY (file_id, channel_id, ts)
);
CREATE INDEX file_shares_channel ON file_shares(channel_id);

-- The files a message carries, as comma-separated IDs. Kept after a file is
-- deleted so the message can show it as a tombstone.
ALTER TABLE messages ADD COLUMN file_ids TEXT NOT NULL DEFAULT '';
