# Hotchkiss-io

Meta Note: This project delivers the hotchkiss-io website so fundamentally this project and the site itself are intertwined.

## Goals
- Central place for Christopher Hotchkiss aka chotchki (me) to present himself to the world, the desired content:
  - Showcase of the projects I've done.
  - Resume since I still like to be gainfully employeed

- Why my own site? I currently have content on github and tons of unposted projects / content. I'd really like to share it but I hate that someone else ends up owning the experience.

- Self hosted, I've run my own website for years on my own hardware and I prefer it that way!
- Self contained, I don't want to depend on external services more than I need to, right now this is:
  - Let's Encrypt for certs
  - Cloudflare for Dynamic DNS — also serves public-IPv4 discovery via `1.1.1.1/cdn-cgi/trace` (was `ifconfig.me`; folded into the Cloudflare dependency we already have, 2026-05)

### Content/Features (current and TBD)
- Projects should support showing the PARTICULAR project type.
  - OpenSCAD should show models
    - The code should be availible with an auto generated lower res stl
    - Need a way to easily bulk load my countless prints
  - Software should show what it does
    - Its okay to link out to GitHub but I want to have the front door since its MY stuff

- Mini Blog — "not super important" but the lack of one might be exactly why I never post; v1 spec below ("Mini Blog (v1)").
- Analytics, who is scraping my site? — **v1 shipped 2026-05** (`/admin/analytics`, admin-only: request log + per-day / top-paths / top-user-agents / distinct-IP aggregates). See PLAN.md Phase 7.
- Backups, the more content that's added the more intrinsic value it has
- Would like to add features that are restricted to the family/approved people
  - I run various services that are non public

## Current site's pain
- ~~deployment is fragile, unsure if I should finally move to docker~~ — **solved 2026-05**: `git push origin main` → post-receive hook on the mini builds, atomic-swaps the `.app`, restarts the LaunchAgent. No docker, no copying stuff around. (See PLAN.md Phase 0.)
- What should be the landing page? that's always hard
- No mini blog
- Mobile posting is too hard, I am very open to enabling a PWA version to enable easier posting
  - easier == I can add an annoucement, attach a couple photos from a phone with a nice interface
- too experimental? I'm mixed on this because this site is also a source of experiments for me
  - I'm proud of passkeys with htmx
  - I like sqlite as a storage mechanism for content but I know it won't scale if I start really loading content

## Mini Blog (v1)

**Goal.** A `/blog` surface that lists posts as cards (cover + date + title + excerpt), so chotchki has somewhere to put short writing without committing to a posting cadence. Slice (a) of the "mini blog + mobile-posting editor facelift" arc — the editor facelift is slice (b), separate SPEC pass later.

**Why now.** The site's "self-hosted, own the experience" thesis is undermined every time there's something to say and it lands on someone else's platform because posting here is too painful. Slice (a) creates the smallest real surface to dogfood the facelift against — the expectation is that testing slice (a) on a phone will surface the editor pain that justifies slice (b).

### Model
- Posts are `content_pages` rows whose parent is a new `blog` special_page (mirrors `projects`; new migration `0010_DMLBlogSpecialPage.sql`).
- Live on save — no `published` column. Saved = published. "Drafts" are unsaved work in the editor.
- `page_creation_date` is the canonical post date; `page_modified_date` is editorial.
- `page_cover_attachment_id` is the card cover.

### URLs
- `/blog` — index, newest first.
- `/blog/<slug>` — single post; `page_name` is the slug (uniqueness via `UNIQUE(parent_page_id, page_name)`).
- `/pages/blog/<slug>` — unchanged, still works via the content_pages tree walk.
- `/blog/feed.xml` — Atom feed.

### Index UX
- Cards: cover image (with a sensible fallback when absent), date, title, excerpt.
- Excerpt = first paragraph of `page_markdown`, formatting stripped, truncated to ~200 chars.
- Empty state: "No posts yet."

### Feed
- Atom 1.0, posts ordered by `page_creation_date` desc, capped at the 50 most recent.
- `<link rel="alternate" type="application/atom+xml">` in the layout head on `/blog` and post pages.

### PWA (minimal)
- `manifest.webmanifest`: `name`, `short_name`, `start_url=/`, `display=standalone`, theme + background colors.
- Icon set under `assets/images/`, pre-rendered from `HotchkissLogo.svg` and committed: 192×192, 512×512, 180×180 (apple-touch-icon), 512×512 maskable.
- No service worker. No offline. "Add to Home Screen" works on iOS Safari and Chrome — that's the install story.
- `<link rel="manifest">` and `<link rel="apple-touch-icon">` in the layout head.

### Editor (single change in this phase)
- Add `capture="environment"` to the attachment upload `<input type="file">` so phones offer Camera-or-Library. One attribute. Every other editor change is slice (b).

### Out of scope (deliberate)
- Editor facelift / autosave / toolbar / drag-paste — slice (b).
- Comments, reactions, social sharing — site ethos is one-way publishing.
- Drafts / scheduled posts — revisit if cadence demands.
- Tags / categories beyond the existing (unused) `page_category` — punt until a real need shows up.
- Heavier PWA (service worker, offline compose, queued attachment upload, push, install prompts) — likely revisited as part of slice (b). The editor is the probable forcing function, not connectivity: a mobile compose flow that wants background save / queued uploads / native-feeling install is what pushes past a static manifest.
