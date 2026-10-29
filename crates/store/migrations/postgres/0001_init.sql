-- Postgres twin of migrations/sqlite. Keep the two in step: same tables,
-- columns and constraints. Differences: BOOLEAN/BIGINT types, `COLLATE "C"`
-- on IDs and timestamps so they sort byte-wise like SQLite's text, and an
-- explicit `rowid` where SQLite's implicit one orders rows by insertion.

CREATE TABLE workspaces (
    id          TEXT COLLATE "C" PRIMARY KEY,
    name        TEXT COLLATE "C" NOT NULL,
    domain      TEXT COLLATE "C" NOT NULL UNIQUE,
    created     BIGINT NOT NULL
);

-- A person who can log in; global across workspaces.
CREATE TABLE accounts (
    id            TEXT COLLATE "C" PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL,
    created       BIGINT NOT NULL
);

-- How an account authenticates. provider = 'password' today; 'oidc:<issuer>' later.
CREATE TABLE identities (
    id           TEXT COLLATE "C" PRIMARY KEY,
    account_id   TEXT COLLATE "C" NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider     TEXT NOT NULL,
    subject      TEXT NOT NULL,
    secret_hash  TEXT,
    created      BIGINT NOT NULL,
    UNIQUE (provider, subject)
);

CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,
    account_id  TEXT COLLATE "C" NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created     BIGINT NOT NULL,
    expires     BIGINT NOT NULL
);
CREATE INDEX sessions_account ON sessions(account_id);

-- Per-workspace profile (U...). Bot users have no account.
CREATE TABLE users (
    id            TEXT COLLATE "C" PRIMARY KEY,
    team_id       TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    account_id    TEXT COLLATE "C" REFERENCES accounts(id) ON DELETE SET NULL,
    name          TEXT COLLATE "C" NOT NULL,
    real_name     TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    email         TEXT,
    is_bot        BOOLEAN NOT NULL DEFAULT FALSE,
    is_admin      BOOLEAN NOT NULL DEFAULT FALSE,
    is_owner      BOOLEAN NOT NULL DEFAULT FALSE,
    deleted       BOOLEAN NOT NULL DEFAULT FALSE,
    tz            TEXT NOT NULL DEFAULT 'UTC',
    created       BIGINT NOT NULL,
    UNIQUE (team_id, name),
    UNIQUE (team_id, account_id)
);

-- Slack API tokens (xoxb-, xoxp-, xapp-). Plaintext so the admin UI can show them.
CREATE TABLE api_tokens (
    token    TEXT PRIMARY KEY,
    kind     TEXT NOT NULL,
    team_id  TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id  TEXT COLLATE "C" REFERENCES users(id) ON DELETE CASCADE,
    app_id   TEXT COLLATE "C",
    bot_id   TEXT COLLATE "C",
    scopes   TEXT NOT NULL DEFAULT '',
    created  BIGINT NOT NULL,
    revoked  BOOLEAN NOT NULL DEFAULT FALSE,
    rowid    BIGINT GENERATED ALWAYS AS IDENTITY
);
