CREATE TABLE IF NOT EXISTS attachments (
    attachment_id           INTEGER     PRIMARY KEY AUTOINCREMENT,
    page_id                 INTEGER     NOT NULL,
    attachment_name         text        NOT NULL,
    mime_type               text        NOT NULL,
    attachment_content      blob        NOT NULL,
    UNIQUE (page_id, attachment_name),
    FOREIGN KEY (page_id) REFERENCES content_pages (page_id)
        ON DELETE CASCADE ON UPDATE CASCADE
);