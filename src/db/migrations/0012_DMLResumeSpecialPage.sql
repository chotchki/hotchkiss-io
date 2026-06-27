-- The `resume` special page: reserves /resume, marks it undeletable, and makes
-- /pages/resume redirect to /resume (mirrors `projects` 0007 / `blog` 0010).
INSERT INTO content_pages (
    page_name,
    page_markdown,
    page_order,
    special_page
) VALUES (
    'resume',
    '/resume',
    -1,
    true
);

-- An empty child to author the résumé into. `/resume` renders the newest child;
-- chris pastes the résumé via /resume?edit. Seeded empty (no content lives in a
-- migration) purely to bootstrap authoring so there's always a page to edit.
INSERT INTO content_pages (
    parent_page_id,
    page_name,
    page_markdown,
    page_order,
    special_page
)
SELECT page_id, 'resume', '', 0, false
FROM content_pages
WHERE page_name = 'resume' AND parent_page_id IS NULL;
