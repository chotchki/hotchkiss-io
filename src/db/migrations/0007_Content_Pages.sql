CREATE TABLE IF NOT EXISTS content_pages (
    page_id                     INTEGER     PRIMARY KEY AUTOINCREMENT,
    parent_page_id              INTEGER     NULL,
    page_name                   text        NOT NULL,
    page_category               text        NULL,
    page_markdown               text        NOT NULL,
    page_cover_attachment_id    INTEGER     NULL,
    page_order                  INTEGER     NOT NULL DEFAULT 0,
    page_creation_date          text        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    page_modified_date          text        NOT NULL DEFAULT CURRENT_TIMESTAMP,
    special_page                INTEGER     NOT NULL DEFAULT 0,
    UNIQUE (parent_page_id, page_name),
    FOREIGN KEY (parent_page_id) REFERENCES content_pages (page_id)
        ON DELETE SET NULL ON UPDATE CASCADE,
    FOREIGN KEY (page_cover_attachment_id) REFERENCES attachments (attachment_id)
        ON DELETE SET NULL ON UPDATE CASCADE
);