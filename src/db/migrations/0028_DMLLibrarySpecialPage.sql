-- The `library` special page: reserves /library for the Family library, marks it
-- undeletable, and makes /pages/library redirect to /library (mirrors 3d 0023 /
-- projects 0007 / blog 0010 / resume 0012). THE ONE GATED SEED: min_role='Family'
-- on this row buys the whole section — the nav tab shows only to Family+ (role-aware
-- TopBar), the /library code routes read this row for their sign-in gate, and the
-- ancestor scan hides every book page under it. Its children are section pages
-- (audiobooks now; manga/video later — authored, not seeded). Seeded page_order -1
-- like the other content tabs (drag-reorder in Manage Pages to place it in the nav).
INSERT INTO content_pages (
    page_name,
    page_markdown,
    page_order,
    special_page,
    min_role
) VALUES (
    'library',
    '/library',
    -1,
    true,
    'Family'
);
