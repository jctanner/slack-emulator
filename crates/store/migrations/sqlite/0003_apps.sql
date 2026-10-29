-- Apps, defined by Slack app manifests and installed into their workspace.
CREATE TABLE apps (
    id                    TEXT PRIMARY KEY,
    team_id               TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name                  TEXT NOT NULL,
    creator               TEXT,
    created               INTEGER NOT NULL,
    updated               INTEGER NOT NULL,
    -- The manifest as JSON, exactly as last saved.
    manifest              TEXT NOT NULL,
    client_id             TEXT NOT NULL UNIQUE,
    client_secret         TEXT NOT NULL,
    signing_secret        TEXT NOT NULL,
    verification_token    TEXT NOT NULL,
    -- Set once installed.
    bot_user_id           TEXT REFERENCES users(id) ON DELETE SET NULL,
    bot_id                TEXT UNIQUE,
    installed_by          TEXT,
    installed_at          INTEGER,
    -- The events request URL that last passed url_verification.
    verified_request_url  TEXT
);
CREATE INDEX apps_team ON apps(team_id);

ALTER TABLE api_tokens ADD COLUMN name TEXT;
CREATE INDEX api_tokens_app ON api_tokens(app_id);
