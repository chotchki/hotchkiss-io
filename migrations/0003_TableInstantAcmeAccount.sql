CREATE TABLE IF NOT EXISTS instant_acme_domains (
    domain text NOT NULL,
    account_credentials text NOT NULL,
    PRIMARY KEY (domain)
);