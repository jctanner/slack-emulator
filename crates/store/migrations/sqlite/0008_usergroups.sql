-- User groups (usergroups.*), mentioned as <!subteam^S…|@handle>.
CREATE TABLE usergroups (
    id           TEXT PRIMARY KEY,
    team_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    handle       TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    -- Default channels, comma-separated.
    channels     TEXT NOT NULL DEFAULT '',
    created_by   TEXT NOT NULL,
    updated_by   TEXT NOT NULL,
    created      INTEGER NOT NULL,
    updated      INTEGER NOT NULL,
    -- Set while disabled (Slack's date_delete).
    disabled_at  INTEGER,
    deleted_by   TEXT,
    UNIQUE (team_id, name),
    UNIQUE (team_id, handle)
);

CREATE TABLE usergroup_members (
    usergroup_id  TEXT NOT NULL REFERENCES usergroups(id) ON DELETE CASCADE,
    user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (usergroup_id, user_id)
);
CREATE INDEX usergroup_members_user ON usergroup_members(user_id);
