CREATE TABLE IF NOT EXISTS attachments (
    parent_page_name text NOT NULL,
    attachment_name text NOT NULL,
    mime_type text NOT NULL,
    attachment_content blob NOT NULL,
    PRIMARY KEY (parent_page_name, attachment_name),
    FOREIGN KEY (parent_page_name) REFERENCES content_pages(page_name)
        ON DELETE CASCADE ON UPDATE CASCADE
);