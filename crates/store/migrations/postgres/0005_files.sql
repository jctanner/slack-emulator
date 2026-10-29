-- Files (files.getUploadURLExternal / completeUploadExternal). Contents live
-- in their own table so listing files never reads the bytes.
CREATE TABLE files (
    id            TEXT COLLATE "C" PRIMARY KEY,
    team_id       TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id       TEXT COLLATE "C" NOT NULL,
    bot_id        TEXT COLLATE "C",
    name          TEXT NOT NULL,
    title         TEXT NOT NULL,
    mimetype      TEXT NOT NULL,
    filetype      TEXT NOT NULL,
    -- Set for text snippets (`snippet_type`).
    snippet_type  TEXT,
    alt_txt       TEXT,
    size          BIGINT NOT NULL,
    created       BIGINT NOT NULL,
    -- Secret part of the upload URL, until the bytes arrive.
    upload_token  TEXT UNIQUE,
    uploaded      BOOLEAN NOT NULL DEFAULT FALSE,
    -- completeUploadExternal has been called.
    completed     BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX files_team ON files(team_id, created);

CREATE TABLE file_contents (
    file_id  TEXT COLLATE "C" PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    data     BYTEA NOT NULL
);

-- Where a file was shared (one row per message it was posted with).
CREATE TABLE file_shares (
    file_id     TEXT COLLATE "C" NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    channel_id  TEXT COLLATE "C" NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ts          TEXT COLLATE "C" NOT NULL,
    PRIMARY KEY (file_id, channel_id, ts)
);
CREATE INDEX file_shares_channel ON file_shares(channel_id);

-- The files a message carries, as comma-separated IDs. Kept after a file is
-- deleted so the message can show it as a tombstone.
ALTER TABLE messages ADD COLUMN file_ids TEXT NOT NULL DEFAULT '';
