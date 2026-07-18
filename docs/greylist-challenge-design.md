# Behavioral greylisting + the cat-toll challenge

Design + rationale for the abusive-IP greylist and the proof-of-work interstitial (Phase CX). This is the single source for WHY the pieces are shaped the way they are — the code comments carry the how, PLAN.md carries the task breakdown, this carries the decisions and the honest limits.

**Scope:** this is annoyance-and-stats defense, NOT load defense. The site is a single Rust binary on gigabit fiber that already shrugs off the traffic — the goal is to stop rewarding scrapers and to stop the junk distorting the analytics (the UA-based `is_bot` classifier counts a spoofed "Safari on Mac" scraper as a HUMAN, so the headline numbers lie). What's explicitly NOT the job: rate-limiting for capacity, blocking a determined adversary who'll purpose-build a solver, or protecting anything a logged-in admin touches.

## The problem, concretely

30 days of `request_log`: ~36k requests the UA heuristic calls bots, plus an unknown slice of UA-spoofing scrapers hiding in the "human" 53k (the `.php` / `wp-login` / `.env` probes on a site that has never served PHP are the tell). Two costs, and only two: it's annoying, and it poisons every success-based number on the dashboard. So the fix targets exactly those — make the abuse unrewarding, and give the stats a signal that UA-spoofing can't fake.

## Threat model — who this actually stops

Being honest about the adversary, because it sets how much machinery is worth building.

- **Dumb HTTP scrapers (the bulk).** `curl` / `python-requests` / `go-http` / the wordlist sprayers. They don't run JS and don't hold cookies. A JS+cookie gate stops them DEAD, whatever the challenge does internally. This is ~all of the 36k (that I've seen so far)
- **Headless-browser scrapers (the tail).** Playwright / Puppeteer run JS, canvas and cookies. The challenge doesn't STOP them — it taxes and annoys them, and a bespoke kernel means the off-the-shelf Anubis solvers (there are several by now) don't apply.
- **NOT in scope: a determined botnet purpose-building a solver for this one personal site.** Nobody's doing that. Design choices that would only matter against that adversary (true memory-hardness, per-request slow-hashing) are deliberately skipped — see the honest-limits section.

The takeaway that drives everything below: the JS+cookie gate is what does the real work. The proof-of-work is the same energy as the blame-the-cat 404 plus bespokeness (no stock solver). Sizing the crypto as if it were the defense would be effort spent on a threat that isn't ours.

## The pipeline

```d2
direction: right
req: Request from IP {shape: oval}
cleared: {label: "cleared?\n(valid clearance cookie,\nauth session, or API key)"; shape: diamond}
grey: {label: "IP on active\ngreylist?"; shape: diamond}
exempt: {label: "exempt path?\n(/challenge, /robots.txt,\n/.well-known, static chrome)"; shape: diamond}
serve: Serve normally {shape: rectangle}
toll: "429 + static cat-toll interstitial" {shape: rectangle}

req -> cleared
cleared -> serve: yes
cleared -> grey: no
grey -> serve: no
grey -> exempt: yes
exempt -> serve: yes
exempt -> toll: no

sweep: {
  label: "Detection sweep (periodic, off the request path)"
  style.stroke-dash: 3
  log: request_log {shape: cylinder}
  rules: "behavioral rules\n(signature-probe / 404-burst / flood)"
  fcrdns: "FCrDNS check\n(verified crawler? -> exempt)"
  greylist: greylist table {shape: cylinder}
  log -> rules -> fcrdns -> greylist
}
sweep.greylist -> grey: feeds active set
```

Two independent loops. The **request path** (top) is synchronous and cheap — three in-memory checks and either serve or toll. The **detection sweep** (bottom) runs periodically OFF the request path, reads the access log the site already keeps, and maintains the greylist the request path reads. Nothing in the hot path does a DNS lookup or a table scan.

## Detection — behavior, not identity

An IP earns the greylist by what it DOES, evaluated over `request_log` on a timer (not instantly — a few minutes' lag is fine and keeps it off the hot path). Rules, roughly in order of confidence:

- **R1 — signature probe (≥2 hits).** Error responses (4xx/5xx) to paths this site never serves (`*.php`, `/wp-*`, `.env`, `/.git`, phpMyAdmin, the phpunit-RCE + `/cgi-bin/luci` wordlists), UA-BLIND: a request claiming to be Googlebot while fetching `wp-login.php` is a liar. The WORKHORSE — verified against a 56-day / 147k-request snapshot to have ZERO false positives (no signature pattern ever matched a served `status < 400` path), and it catches 760 IPs.
- **R2 — distinct-404 burst (≥40 over the ~24h window).** A UA-spoofing scraper walking a wordlist of dead paths. Tuned HIGH on purpose — a backstop (99% of trippers already trip R1) set clear of the operator's own home IP, which carried 20 distinct 404s over 56 days.
- **R3 — flood (≥1000 over the ~24h window).** The blunt fallback for high-volume abuse that's neither signature- nor 404-shaped; above any human (the operator's busiest day was ~366 requests).

R2 and R3 EXEMPT verified search crawlers (below); R1 does not (nothing legit probes PHP). Thresholds were tuned 2026-07-05 against a real `request_log` snapshot — R1 does the work, R2/R3 are conservative backstops. The rules score over a pure `ip_features(pool, ip, window) -> IpFeatures` — one place, unit-tested — and a pluggable `score(features) -> Verdict`. That split is deliberate: the hand-tuned weights ARE a linear classifier, and when they start losing (they won't for years — mass scanners aren't adapting to this site specifically) swapping fitted weights for hand ones is a one-function change, not a rewrite. The greylist rows carry their reason + evidence, so the training set accumulates for free — with the honest caveat that it's rule-labeled, so a fitted model would learn the rules' biases unless the curated-refinement panel (below) keeps a human in the loop.

The sweep evaluates a ~24h window and skips loopback + RFC1918 so a dev / LAN client can't greylist itself.

### Operator auto-allowlist — the server's own public IP (Phase DU)

The mini self-hosts on the operator's residential connection, so its **own public IP** — the one `IpProviderService` already tracks and feeds Cloudflare for the DNS updates — **is the operator's browsing IP** (traffic from the home network hairpins through the router NAT and the mini sees it as the shared public IP). Left alone, the greylist tolls the operator: a busy home day can trip R3, and a manual release doesn't stick because the next 15-min sweep re-adds it. So the server's tracked public IP(s) are **auto-exempt** at BOTH enforcement points:

- **Detection** — the sweep skips scoring an allowlisted IP (it's not covered by the loopback/RFC1918 skip, since it's a genuine public IP), so it's never added; and each pass **releases** any stale/pinned row for it (e.g. one carried in a prod→beta snapshot) so the table + admin view stay clean.
- **Enforcement** — `GreylistSet::is_greylisted` returns false for an allowlisted IP: the allowlist **wins over any entry**, snapshot OR manual pin, so the operator's network can never be tolled even by a stale row racing the next sweep.

It's **zero config and self-maintaining**: a detached coordinator task subscribes to the same IP broadcast the DNS service reads and calls `GreylistSet::set_public_ips` on every change, so a residential IP rotation updates DNS *and* the allowlist together. (In `debug_assertions` the tracked IP is forced to `127.0.0.1`, already covered by the RFC skip — harmless.) The admin `/admin/greylist` page shows the exempt IP(s) so it's visible WHY they're never tolled. This deliberately covers ONLY the home network; from any other network (cellular, an office) the operator just **authenticates** — an `is_authenticated()` session is never tolled — so an arbitrary-IP/CIDR config allowlist was decided OUT as unnecessary surface.

### Crawler safety — FCrDNS, not a UA allowlist

A real search crawler CAN plausibly trip R2 — after a site restructure Googlebot re-crawls dead URLs for years (like /pages/Resume) and eats a 404 burst. A UA string is no protection (it's the first thing a scraper spoofs). So before R2/R3 auto-greylists a crawler-claiming IP, the sweep does **forward-confirmed reverse DNS**: reverse the IP, check the PTR ends in a known crawler suffix (`googlebot.com`, `search.msn.com`, `crawl.yahoo.net`, …), then forward-resolve that name and confirm it maps back to the same IP. A spoofed UA can't fake DNS it doesn't control. Verified crawlers are never auto-greylisted, and the exemption is recorded so it's visible WHY an IP that looks noisy was spared.

This reuses the async DNS resolver already in the ACME path (it does the PTR reverse lookup AND the forward A confirm), so it's a normal async lookup with a timeout — no `spawn_blocking` thread needed. Results cached, and a DNS failure SKIPS that tick's verdict rather than greylisting on incomplete info (fail toward NOT punishing a maybe-legit crawler). The resolver is injected so tests don't touch the network. This is the sweep — never the request path — so its latency is invisible to visitors.

## The challenge kernel

The interstitial is a proof-of-work toll (the concept Anubis popularized), reimplemented so no stock solver applies and so it tells a story — the toll runs the site's own [memory-intensive hashing scheme](https://hotchkiss.io/blog/memory-intensive-secure-password-hashing) over a sarcastic image the visitor "has" to paint to prove they're a browser.

**Construction:**

- `image_digest = slow_chain(image_bytes)` — walk the toll image pixel by pixel, `h[i] = H(rgba[i] ‖ h[i-1])`, store each `h[i]`, then fold the array in REVERSE for the final digest (the reverse pass is what forces holding the whole array — the mechanism from the blog post). This depends ONLY on the image, so it's constant until the art changes (a commit + deploy).
- `answer = HMAC(seed, image_digest)` — the only per-request step, cheap. `seed` is NOT stored server-side; it's a signed, self-timestamping token (see "Stateless challenge" below), so the whole challenge holds zero per-challenge state.

**Why the seed rides the HMAC and not the chain:** it lets the server precompute `image_digest` ONCE at boot (the art is a committed asset; background `spawn_blocking`) and store just the 32-byte result, so issuing a challenge is `HMAC(new seed, cached digest)` and verifying is an O(1) constant-time compare. The client still has to run the full slow chain on its first solve. Yes, a client COULD cache `image_digest` too (it's the same image-only function) and skip the slow part on a second solve — but that's pointless for the actual threat: a cleared client rides its cookie and never re-solves, and the only adversary who benefits from caching is the determined botnet that's explicitly out of scope. Threading the seed INTO `h[0]` instead makes the whole chain per-request (server pays the O(n) at gated issue-time, no precompute for anyone) — that's the config flip if the threat ever changes, not a rearchitecture.

**Stateless challenge — a signed seed, no store.** The seed is `HMAC(server_key, inner_seed ‖ timestamp ‖ digest_version)`. `/challenge/new` draws a random `inner_seed`, stamps the current `timestamp` + the live `digest_version`, computes the seed, and returns `{inner_seed, timestamp, digest_version, seed}` (the image itself is a separately-cached static asset referenced by version — NOT re-sent per call). The client solves and submits `{inner_seed, timestamp, digest_version, answer}`. Verify RECOMPUTES the seed from the echoed fields — which integrity-protects all three, since tampering any of them changes the seed and breaks the answer match — checks the `timestamp` is fresh, looks up that version's cached `image_digest`, recomputes `HMAC(seed, image_digest)` and constant-time-compares. The server-key HMAC is what makes the seed unforgeable (a client can't mint a seed the server didn't issue — it doesn't have the key), and the timestamp riding INSIDE the HMAC is what makes it un-post-dateable (the client is locked to the exact `(inner_seed, timestamp)` the server chose). Net: the server holds NO per-challenge state — it survives a restart, `/challenge/new` is O(1) (a random draw + one HMAC), and there's nothing to prune. The cost is single-use — see the honest limits.

**The image dimensions ARE the work factor.** 320×320 is ~102k pixels ≈ the blog's recommended 100k iterations; 512×512 is ~262k. Pick the picture size, pick the iteration count — the difficulty dial and the art are the same knob. A solve is a few hundred ms to ~1s of client CPU and 2–8 MB of held array, which is a fine human toll and a fine browser footprint. First art is the Blazing Saddles desert tollbooth ("you'd need a shitload of dimes" — a pointless toll planted where you could just ride around it, which IS the greylist).

**The shipped+hashed artifact is DERIVED raw RGBA, not the source file.** The art is a committed PNG at `assets/greylist/toll.png` (rust-embed bundles it into the binary; the source + the `build/greylist/make-toll-image.sh` regen script live under `build/greylist/`, outside `assets/` like the SVG icon sources). At boot the server DECODES it, forces every pixel fully opaque, and emits the raw RGBA byte buffer — that buffer is what's served (as a static asset) AND what the slow chain hashes. We do NOT ship the PNG for the client to re-decode: even a PNG `drawImage` can color-manage, and a JPEG decode is lossy + platform-varying (IDCT / chroma upsampling differ) — either would make the client's bytes diverge from the server's and fail every honest solve. Raw RGBA in, `new ImageData(...)` + `putImageData` for the paint, hash the bytes directly: identical on every device by construction. The downscale height is the difficulty knob — pixel count = iteration count; the shipped 589×320 art is ~188k iterations (~1.9× the blog's 100k).

**Serving it, without a self-inflicted DoS:**

- The 429 interstitial is self contained STATIC HTML+JS (cheap, no per-hit compute). A dumb bot retrying the URL gets a static 429 forever and never costs the server a hash.
- The JS calls `GET /challenge/new` to get its token when it's about to solve. That endpoint is O(1) (a random draw + one HMAC — the `image_digest` is already cached per rotation and the image is a static asset), so hammering it costs nothing and there's NO rate-limiting in v1. Trigger to add one: bandwidth abuse of the image asset (which cache headers already blunt).
- The chain is SEQUENTIAL (each pixel depends on the prior hash), so it can't be split across workers the way Anubis parallelizes independent nonces. One Web Worker runs the whole pass and posts progress back to paint the image + move the bar (off the main thread so the page doesn't jank), using a small WASM or pure-JS SHA-256 — NOT `crypto.subtle` per pixel, whose async per-call overhead makes 100k tiny sequential hashes crawl.
- **Digest-version pinning:** `digest_version` is the committed art's content hash, bound INTO the signed seed and echoed back. There's no runtime rotation — new art is a commit + deploy, and the restart recomputes the single digest — so the only version change is across a deploy: a token issued just before and verified just after hashed the OLD art → clean mismatch → the client retries against the new art (the short freshness window kills stale tokens fast anyway). Binding it into the seed HMAC means the client can't claim a version it wasn't issued. (A rotating invisible watermark to re-invalidate a cached digest was considered and DROPPED — extra server code for the out-of-scope digest-caching threat; if that ever bites, the answer is stronger measures, not marginal pixel-rotation.)

**Status code: 429 + `Retry-After`, not 200.** Anubis serves 200 because scrapers want one and stop — but a 200 pollutes every success-based number on the dashboard (the exact stat distortion this whole thing exists to fix), and if a false positive ever hit a real crawler, a 200 challenge page carries the risk of getting real URLs mis-cached. 429 is the honest "back off", challenged hits fall OUT of the `status < 400` success filters automatically, and it's the crawl-safe degrade (Google treats 429 as temporary, retry later). The interstitial is `noindex,nofollow` regardless.

**Interstitial copy** (verbatim — chris's, don't restyle):
- Headline: "As a suspected bot, you need to pay a cryptographic toll to reach hotchkiss.io"
- Subtitle: "Dimes not accepted."
- Footer: credit the film — a small "Image: *Blazing Saddles* (1974), Warner Bros." line at the bottom of the page (attribution, and it reinforces the parody/fair-use posture).

## The clearance artifact

A solved challenge mints a clearance cookie: a signed bearer token `HMAC(server_key, expiry)` (new `crypto_keys` id 4 — 1 is the session key, 2 the media-URL HMAC, 3 the API-key pepper), 7-day expiry, `HttpOnly` + `Secure` + `SameSite=Lax` like the session cookie. The request path verifies it in memory — recompute the MAC, check the expiry — no DB hit.

It is deliberately NOT IP-bound. Mobile IPs change mid-session (cell handoff, wifi↔cell), so binding the clearance to the issuing IP would re-challenge a legit phone user the instant their address flipped — punishing exactly the CGNAT-false-positive human we most want to treat gently. The cost of not binding: the cookie is a bearer token, so a solver COULD share one cleared cookie to a fleet and skip the toll for 7 days. That's the same botnet-mass-clear already conceded out of scope (now a 7-day window vs the challenge's 2-min replay window — a real downgrade, but only against an adversary that isn't ours), and `HttpOnly` + `Secure` keep the theft surface to malware on the client, not XSS or the wire. Usability for the false-positive human wins over resistance to an out-of-scope attack.

Passing is its OWN signal, and a stronger one than the UA guess: a cleared client actually ran the JS and solved the toll (stamp it), where `is_bot` only ever guessed from a spoofable string. The inverse — a client presenting a valid clearance that KEEPS scanning signature paths — is the cleanest bot signature in the system (a headless scraper that paid the toll), and it's the natural escalation trigger. That escalation is deferred (see below), but the clearance record is built to feed it.

## Analytics integration

Every challenged request gets stamped `challenged = 1` AND `is_bot = 1` in `request_log` (via a response extension the logging middleware reads — the middleware is already outermost, so it sees the toll response). That's the point of the whole exercise for the stats: challenged traffic is provably-not-human, no UA guessing, so the Humans/Bots chips finally tell the truth. `/admin/greylist` lists the active entries with reason + evidence, challenges served and clearances; the IP drill-down grows a "Greylist this IP" button for manual pins (which never expire until released — unlike the auto entries, whose expiry slides: extended while abuse continues, lapsed after 7 days quiet).

## What this deliberately does NOT do

The honest limits, so nobody mistakes the toll for more than it is:

- **It is NOT memory-hard in the Argon2 sense — it's memory-FAVORING.** The reverse-fold forces the naive solver to hold the whole array, but "reverse" is a fixed, predictable permutation and the chain is cheaply recomputable, so it admits the standard time-memory tradeoff (checkpoint every k-th hash, recompute the rest: ~√n memory for a ~√n time cost). Real memory-hardness needs DATA-dependent addressing (the next index derived from the value just computed, like scrypt's ROMix), which the reversal isn't. We don't add it because nobody's building silicon to crack a personal site's toll, and it would cost the pretty scan-order paint (the fix, if ever wanted: hash in `H(...) mod n` order, paint in scan order).
- **The canvas is not FORCEABLE, and we don't pretend it is.** `putImageData`→`getImageData` is a byte-exact memcpy, so a solver hashes the shipped bytes without ever touching a canvas — deterministic readback and mandatory-canvas are mutually exclusive, and determinism wins (every browser must produce the same digest). The canvas write is the show (paint the toll) plus a "did JS actually run" signal, not a cryptographic browser-proof. The digest is computed over the bytes the server SHIPS (image kept fully opaque so there's no rasterization and no fingerprinting surface at all), not a canvas readback — reassurance for the "will this trip browser fingerprinting" worry: no, because nothing rasterizes.
- **The precompute advantage is symmetric.** Anything the server precomputes once per rotation, an attacker can too. That's inherent to the image-only slow part and it's an accepted trade for a near-free server (see the kernel section).
- **No single-use — a solved challenge is replayable within the freshness window.** Statelessness means no spent-marker, so a valid `{token, answer}` redeems more than once, from any IP. Kept in check by a SHORT window (a couple minutes, NOT Anubis's 30 — without single-use the window IS the replay bound; it only has to clear a ~1s machine solve plus network). The bigger sharing surface is actually the clearance cookie the solve mints (7-day bearer, not IP-bound — see the clearance section), not the answer; both collapse to the same already-conceded botnet-mass-clear (and it's already cheap via digest caching, so neither adds anything new). True single-use would need a spent-token set — state we deliberately don't keep; if the threat ever tightens it's a small re-add (token id → seen, windowed TTL), small because the window is short.
- **Client IP is the direct socket peer.** The app terminates its own TLS with no proxy in front, so there's no `X-Forwarded-For` to trust or spoof — but it also means IPv4 today (an IPv6 `/64` grouping is moot until there's an AAAA record) and CGNAT lumps many clients behind one IP: greylisting that IP challenges every innocent co-tenant ONCE, and because the clearance isn't IP-bound each then rides their own 7-day cookie (no re-challenge when their address flips). One toll per innocent co-tenant is the accepted cost.
- **Escalation on clear-then-scan is deferred.** The signature's recorded, the response isn't built (revoke + hard block is a v2 lever).

## Beta caveat

The prod→beta snapshot SCRUBS `request_log` for visitor privacy, so the detection sweep finds nothing to act on and the greylist dark-launches EMPTY on beta. The challenge machinery still works there (seed a greylist row by hand or hit `/admin/greylist`'s manual pin), but the auto-detection can only be exercised against prod's real log or a seeded test fixture. To exercise the FULL detection→toll path on beta, hit a signature path a couple times (`curl https://beta.hotchkiss.io/wp-login.php` ×2) then click the admin **Run sweep now** button (release-safe — no `debug_assertions` seam, since beta is a release build): your own IP greylists and the next page load shows the toll. A manual pin of your IP tests the challenge flow instantly, skipping detection. Also: the beta snapshot preserves `crypto_keys` id 2 only, so a new id 4 regenerates on beta — beta clearances won't verify on prod and vice-versa, which is fine (they're separate hosts).

## Deferred levers (with their triggers)

- **Fitted classifier** — when the hand weights visibly lose. The `ip_features` + `score()` seam already exists for it; the rule-labeled training set is already accumulating.
- **Clear-then-scan escalation** — revoke clearance + hard-block. Trigger: a cleared IP keeps probing signature paths.
- **Batched request_log writer** — the sweep + stamp adds no write-path cost today (it reads on a timer), so the existing fire-and-forget insert stands. Trigger: sustained write contention (`SQLITE_BUSY`).
- **IPv6 `/64` grouping** — moot until an AAAA record exists.... come on AT&T
- **IP-range (/24) greylisting** — the tuning snapshot showed a `185.177.72.0/24` scanner farm (8+ IPs, thousands of distinct 404s each), greylisted one IP at a time today. Greylisting the whole /24 after N sibling hits is a cheap future win. Deferred.

## Interactions decided elsewhere

- **`/media/file/` is excluded from `request_log` (Phase DD).** Audio/video streaming means hundreds-to-thousands of range requests per listen, and R3 fires at 1000 req/IP/24h (calibrated on ~366-request human days) — two family listeners behind one home NAT would plausibly greylist their OWN IP, and the rows would swamp the Humans/top-paths signal. Since detection READS `request_log`, the exclusion also means byte-route traffic simply doesn't exist to R1/R2/R3 — which is correct: it's static-asset-class traffic, and every gated fetch is separately authenticated at the route. Honest cost: no byte-route analytics; listens stay visible as page/embed views.
