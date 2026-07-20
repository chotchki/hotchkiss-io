-- The `capture` special page (EB.12): puts the quick-capture lane in the nav as
-- a REAL content tab — reorderable in Manage Pages like the rest — instead of a
-- hardcoded pill in the admin group (which sat fat-finger-close to everything).
-- min_role='Admin' means only an Admin ever sees the tab (role-aware TopBar,
-- the same seam as library's Family gate); /pages/capture redirects to the
-- admin-gated /admin/capture. Seeded page_order 99 to land LAST — drag it
-- wherever it belongs.
INSERT INTO content_pages (
    page_name,
    page_markdown,
    page_order,
    special_page,
    min_role
) VALUES (
    'capture',
    '/admin/capture',
    99,
    true,
    'Admin'
);
