-- App Home tabs: one published view per app and user (`views.publish`).
-- Modals live only in memory, like the client windows they belong to.
CREATE TABLE home_views (
    view_id      TEXT PRIMARY KEY,
    app_id       TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    external_id  TEXT,
    -- The server's view record as JSON.
    data         TEXT NOT NULL,
    UNIQUE (app_id, user_id)
);
CREATE INDEX home_views_external ON home_views(app_id, external_id);
