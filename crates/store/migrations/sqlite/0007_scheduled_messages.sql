-- chat.scheduleMessage: posted by a background task when post_at arrives.
CREATE TABLE scheduled_messages (
    id          TEXT PRIMARY KEY,
    team_id     TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    channel_id  TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    -- Who it posts as: a person (web UI) or an API token's user.
    user_id     TEXT NOT NULL,
    -- The token that scheduled it, if any; it must still be valid at post time.
    token       TEXT,
    app_id      TEXT,
    post_at     INTEGER NOT NULL,
    created     INTEGER NOT NULL,
    -- The chat.postMessage arguments, as JSON.
    args        TEXT NOT NULL
);
CREATE INDEX scheduled_messages_due ON scheduled_messages(post_at);
