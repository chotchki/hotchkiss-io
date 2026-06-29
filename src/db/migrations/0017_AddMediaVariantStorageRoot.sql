-- Phase CJ: multi-drive media storage. Record WHICH configured root each
-- variant's bytes were written to, so the serve route can go straight there
-- (O(1)) instead of statting every root. Nullable on purpose: existing rows, and
-- any variant whose drive has since been moved, fall back to the first-found scan
-- across all roots — so this is a pure HINT, never a hard pointer.
ALTER TABLE media_variant ADD COLUMN storage_root TEXT;
