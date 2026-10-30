-- User groups (usergroups.*), mentioned as <!subteam^S…|@handle>.
CREATE TABLE usergroups (
    id           TEXT COLLATE "C" PRIMARY KEY,
    team_id      TEXT COLLATE "C" NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    handle       TEXT COLLATE "C" NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    -- Default channels, comma-separated.
    channels     TEXT NOT NULL DEFAULT '',
    created_by   TEXT COLLATE "C" NOT NULL,
    updated_by   TEXT COLLATE "C" NOT NULL,
    created      BIGINT NOT NULL,
    updated      BIGINT NOT NULL,
    -- Set while disabled (Slack's date_delete).
    disabled_at  BIGINT,
    deleted_by   TEXT COLLATE "C",
    UNIQUE (team_id, name),
    UNIQUE (team_id, handle)
);

CREATE TABLE usergroup_members (
    usergroup_id  TEXT COLLATE "C" NOT NULL REFERENCES usergroups(id) ON DELETE CASCADE,
    user_id       TEXT COLLATE "C" NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (usergroup_id, user_id)
);
CREATE INDEX usergroup_members_user ON usergroup_members(user_id);
