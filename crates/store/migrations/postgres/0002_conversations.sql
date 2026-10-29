-- Channels (public/private), DMs and group DMs.
CREATE TABLE conversations (
    id                TEXT COLLATE "C" PRIMARY KEY,
    team_id           TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    -- NULL for DMs and group DMs.
    name              TEXT COLLATE "C",
    is_private        BOOLEAN NOT NULL DEFAULT FALSE,
    is_im             BOOLEAN NOT NULL DEFAULT FALSE,
    is_mpim           BOOLEAN NOT NULL DEFAULT FALSE,
    is_general        BOOLEAN NOT NULL DEFAULT FALSE,
    is_archived       BOOLEAN NOT NULL DEFAULT FALSE,
    creator           TEXT COLLATE "C",
    created           BIGINT NOT NULL,
    topic             TEXT NOT NULL DEFAULT '',
    topic_creator     TEXT NOT NULL DEFAULT '',
    topic_last_set    BIGINT NOT NULL DEFAULT 0,
    purpose           TEXT NOT NULL DEFAULT '',
    purpose_creator   TEXT NOT NULL DEFAULT '',
    purpose_last_set  BIGINT NOT NULL DEFAULT 0,
    -- Sorted member IDs joined by ',' for DMs and group DMs, so they can be found by participants.
    dm_key            TEXT COLLATE "C",
    UNIQUE (team_id, name),
    UNIQUE (team_id, dm_key)
);

CREATE TABLE conversation_members (
    conversation_id  TEXT COLLATE "C" NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id          TEXT COLLATE "C" NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined           BIGINT NOT NULL,
    -- ts of the newest message this member has read.
    last_read        TEXT COLLATE "C" NOT NULL DEFAULT '0000000000.000000',
    PRIMARY KEY (conversation_id, user_id)
);
CREATE INDEX conversation_members_user ON conversation_members(user_id);

-- ts is "<10-digit seconds>.<6 digits>"; fixed width so it sorts as text.
CREATE TABLE messages (
    channel_id       TEXT COLLATE "C" NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ts               TEXT COLLATE "C" NOT NULL,
    team_id          TEXT COLLATE "C" NOT NULL,
    user_id          TEXT COLLATE "C",
    bot_id           TEXT COLLATE "C",
    app_id           TEXT COLLATE "C",
    subtype          TEXT,
    text             TEXT NOT NULL DEFAULT '',
    blocks           TEXT,
    attachments      TEXT,
    -- Bot display overrides (chat.postMessage username/icon_*).
    username         TEXT,
    icons            TEXT,
    metadata         TEXT,
    client_msg_id    TEXT,
    -- Set on replies (and on parents once they have replies).
    thread_ts        TEXT COLLATE "C",
    reply_broadcast  BOOLEAN NOT NULL DEFAULT FALSE,
    edited_ts        TEXT COLLATE "C",
    edited_by        TEXT COLLATE "C",
    PRIMARY KEY (channel_id, ts)
);
CREATE INDEX messages_thread ON messages(channel_id, thread_ts, ts);

CREATE TABLE reactions (
    channel_id  TEXT COLLATE "C" NOT NULL,
    ts          TEXT COLLATE "C" NOT NULL,
    name        TEXT NOT NULL,
    user_id     TEXT COLLATE "C" NOT NULL,
    created     BIGINT NOT NULL,
    rowid       BIGINT GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (channel_id, ts, name, user_id),
    FOREIGN KEY (channel_id, ts) REFERENCES messages(channel_id, ts) ON DELETE CASCADE
);
