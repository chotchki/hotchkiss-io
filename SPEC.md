# Hotchkiss-io

Meta Note: This project delivers the hotchkiss-io website so fundamentally this project and the site itself are intertwined.

## Goals
- Central place for Christopher Hotchkiss aka chotchki (me) to present himself to the world, the desired content:
  - Showcase of the projects I've done.
  - Resume since I still like to be gainfully employeed

- Why my own site? I currently have content on github and tons of unposted projects / content. I'd really like to share it but I hate that someone else ends up owning the experience.

- Self hosted, I've run my own website for years on my own hardware and I prefer it that way!
- Self contained, I don't want to depend on external services more than I need to, right now this is:
  - ifconfig.me for ipv4 (I'd REALLY like to remove this)
  - Let's Encrypt for certs 
  - Cloudflare for Dynamic DNS

### Content/Features (current and TBD)
- Projects should support showing the PARTICULAR project type.
  - OpenSCAD should show models
    - The code should be availible with an auto generated lower res stl
    - Need a way to easily bulk load my countless prints
  - Software should show what it does
    - Its okay to link out to GitHub but I want to have the front door since its MY stuff

- Mini Blog (not super important, I don't want to post regular content but that may be because of how hard it is)
- Analytics, who is scraping my site?
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
