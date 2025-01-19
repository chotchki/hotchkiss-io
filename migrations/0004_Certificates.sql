CREATE TABLE IF NOT EXISTS certificates (
    domain text NOT NULL,
    certificate_chain text NOT NULL,
    private_key text NOT NULL,
    PRIMARY KEY (domain)
);