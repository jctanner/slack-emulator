-- Apps, defined by Slack app manifests and installed into their workspace.
CREATE TABLE apps (
    id                    TEXT COLLATE "C" PRIMARY KEY,
    team_id               TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name                  TEXT COLLATE "C" NOT NULL,
    creator               TEXT COLLATE "C",
    created               BIGINT NOT NULL,
    updated               BIGINT NOT NULL,
    -- The manifest as JSON, exactly as last saved.
    manifest              TEXT NOT NULL,
    client_id             TEXT NOT NULL UNIQUE,
    client_secret         TEXT NOT NULL,
    signing_secret        TEXT NOT NULL,
    verification_token    TEXT NOT NULL,
    -- Set once installed.
    bot_user_id           TEXT COLLATE "C" REFERENCES users(id) ON DELETE SET NULL,
    bot_id                TEXT COLLATE "C" UNIQUE,
    installed_by          TEXT COLLATE "C",
    installed_at          BIGINT,
    -- The events request URL that last passed url_verification.
    verified_request_url  TEXT
);
CREATE INDEX apps_team ON apps(team_id);

ALTER TABLE api_tokens ADD COLUMN name TEXT;
CREATE INDEX api_tokens_app ON api_tokens(app_id);
