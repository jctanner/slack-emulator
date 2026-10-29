-- Channels (public/private), DMs and group DMs.
CREATE TABLE conversations (
    id                TEXT PRIMARY KEY,
    team_id           TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    -- NULL for DMs and group DMs.
    name              TEXT,
    is_private        INTEGER NOT NULL DEFAULT 0,
    is_im             INTEGER NOT NULL DEFAULT 0,
    is_mpim           INTEGER NOT NULL DEFAULT 0,
    is_general        INTEGER NOT NULL DEFAULT 0,
    is_archived       INTEGER NOT NULL DEFAULT 0,
    creator           TEXT,
    created           INTEGER NOT NULL,
    topic             TEXT NOT NULL DEFAULT '',
    topic_creator     TEXT NOT NULL DEFAULT '',
    topic_last_set    INTEGER NOT NULL DEFAULT 0,
    purpose           TEXT NOT NULL DEFAULT '',
    purpose_creator   TEXT NOT NULL DEFAULT '',
    purpose_last_set  INTEGER NOT NULL DEFAULT 0,
    -- Sorted member IDs joined by ',' for DMs and group DMs, so they can be found by participants.
    dm_key            TEXT,
    UNIQUE (team_id, name),
    UNIQUE (team_id, dm_key)
);

CREATE TABLE conversation_members (
    conversation_id  TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id          TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined           INTEGER NOT NULL,
    -- ts of the newest message this member has read.
    last_read        TEXT NOT NULL DEFAULT '0000000000.000000',
    PRIMARY KEY (conversation_id, user_id)
);
CREATE INDEX conversation_members_user ON conversation_members(user_id);

-- ts is "<10-digit seconds>.<6 digits>"; fixed width so it sorts as text.
CREATE TABLE messages (
    channel_id       TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ts               TEXT NOT NULL,
    team_id          TEXT NOT NULL,
    user_id          TEXT,
    bot_id           TEXT,
    app_id           TEXT,
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
    thread_ts        TEXT,
    reply_broadcast  INTEGER NOT NULL DEFAULT 0,
    edited_ts        TEXT,
    edited_by        TEXT,
    PRIMARY KEY (channel_id, ts)
);
CREATE INDEX messages_thread ON messages(channel_id, thread_ts, ts);

CREATE TABLE reactions (
    channel_id  TEXT NOT NULL,
    ts          TEXT NOT NULL,
    name        TEXT NOT NULL,
    user_id     TEXT NOT NULL,
    created     INTEGER NOT NULL,
    PRIMARY KEY (channel_id, ts, name, user_id),
    FOREIGN KEY (channel_id, ts) REFERENCES messages(channel_id, ts) ON DELETE CASCADE
);
