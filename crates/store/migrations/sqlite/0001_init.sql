CREATE TABLE workspaces (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    domain      TEXT NOT NULL UNIQUE,
    created     INTEGER NOT NULL
);

-- A person who can log in; global across workspaces.
CREATE TABLE accounts (
    id            TEXT PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL,
    created       INTEGER NOT NULL
);

-- How an account authenticates. provider = 'password' today; 'oidc:<issuer>' later.
CREATE TABLE identities (
    id           TEXT PRIMARY KEY,
    account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider     TEXT NOT NULL,
    subject      TEXT NOT NULL,
    secret_hash  TEXT,
    created      INTEGER NOT NULL,
    UNIQUE (provider, subject)
);

CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created     INTEGER NOT NULL,
    expires     INTEGER NOT NULL
);
CREATE INDEX sessions_account ON sessions(account_id);

-- Per-workspace profile (U...). Bot users have no account.
CREATE TABLE users (
    id            TEXT PRIMARY KEY,
    team_id       TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    account_id    TEXT REFERENCES accounts(id) ON DELETE SET NULL,
    name          TEXT NOT NULL,
    real_name     TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    email         TEXT,
    is_bot        INTEGER NOT NULL DEFAULT 0,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    is_owner      INTEGER NOT NULL DEFAULT 0,
    deleted       INTEGER NOT NULL DEFAULT 0,
    tz            TEXT NOT NULL DEFAULT 'UTC',
    created       INTEGER NOT NULL,
    UNIQUE (team_id, name),
    UNIQUE (team_id, account_id)
);

-- Slack API tokens (xoxb-, xoxp-, xapp-). Plaintext so the admin UI can show them.
CREATE TABLE api_tokens (
    token    TEXT PRIMARY KEY,
    kind     TEXT NOT NULL,
    team_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id  TEXT REFERENCES users(id) ON DELETE CASCADE,
    app_id   TEXT,
    bot_id   TEXT,
    scopes   TEXT NOT NULL DEFAULT '',
    created  INTEGER NOT NULL,
    revoked  INTEGER NOT NULL DEFAULT 0
);
