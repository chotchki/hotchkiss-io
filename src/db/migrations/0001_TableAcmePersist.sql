CREATE TABLE IF NOT EXISTS acme_persist (
    acme_key text NOT NULL,
    acme_value BLOB NOT NULL,
    PRIMARY KEY (acme_key)
);