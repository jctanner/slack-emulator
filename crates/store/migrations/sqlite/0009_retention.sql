-- Message retention: a workspace default (target = the workspace ID) and
-- per-conversation overrides (target = a conversation ID). No row means
-- messages are kept forever.
CREATE TABLE retention_policies (
    target_id  TEXT PRIMARY KEY,
    team_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    days       INTEGER NOT NULL
);
CREATE INDEX retention_policies_team ON retention_policies(team_id);
