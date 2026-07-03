-- The `3d` special page: reserves /3d for the 3D-printing gallery, marks it
-- undeletable, and makes /pages/3d redirect to /3d (mirrors projects 0007 /
-- blog 0010 / resume 0012). Its children are the model pages; the /3d index
-- (web/features/three_d.rs) lists them — a Featured band from the reused
-- pin/`featured` tag above the rest. Seeded page_order -1 like the other content
-- tabs (drag-reorder in Manage Pages to place it in the nav).
INSERT INTO content_pages (
    page_name,
    page_markdown,
    page_order,
    special_page
) VALUES (
    '3d',
    '/3d',
    -1,
    true
);
