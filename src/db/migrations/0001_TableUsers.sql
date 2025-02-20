CREATE TABLE IF NOT EXISTS users (
    display_name text NOT NULL,
    id text NOT NULL,
    keys text NOT NULL,
    app_role text NOT NULL,
    PRIMARY KEY (display_name),
    UNIQUE(id)
);