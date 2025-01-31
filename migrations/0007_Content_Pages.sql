CREATE TABLE IF NOT EXISTS content_pages (
    page_name text NOT NULL,
    page_markdown text NOT NULL,
    page_order INTEGER NOT NULL DEFAULT 0,
    special_page INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (page_name)
);