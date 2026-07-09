# The Home tab — house control for the non-iOS half of the family

The library (Phase DE) proved the shape: a Family-gated top-level tab on the site the whole household already signs into with passkeys. This doc designs the SECOND consumer of that permission foundation — a `/home` tab that lets the non-iOS family members turn lights on and off, adjust the A/C and see what state the house is in. The iOS half already has Apple Home; the Android half has NOTHING, and that asymmetry is the problem. The site is the natural fix because the hard parts (trust tier, gated nav, sign-in gate, role-scoped POSTs, the mini sitting on the LAN) are ALREADY BUILT — what this doc actually decides is the actuation bridge.

## The estate, concretely

"Duloc" is ~30 accessories across 11 rooms: blinds (5+), lamps and lights (10+), two locks (front door, garage), ceiling fans, bathroom heaters, air filters, a garage door, HomePods in nearly every room, and TWO Daikin minisplits. Almost all of it is NATIVE HomeKit — paired straight into Apple Home, no vendor hub with a usable API behind it. The two exceptions:

- **The Daikin minisplits** run deliberately-imported Romanian controllers that speak PLAIN LOCAL HTTP (the American equivalents are cloud-only, which is exactly why they were imported). Today Homebridge fronts them into Apple Home via `homebridge-daikin-local`.
- **Homebridge** itself (on the mini) fronts ONLY those Daikins plus some dummy switches — and its quality is a standing frustration. Its footprint is small enough to eventually retire (see the endgame).

A third fact from the Home app screenshot that shapes the UI: the estate has a **No Response epidemic** (a bathroom heater, a fan 9h stale, the front-door lock THREE WEEKS unresponsive). Any panel we build must surface staleness honestly — a control page that shows a confident "Locked" for a lock that hasn't answered in weeks is worse than no page.

## Design principles

1. **LAN-first, no vendor clouds, ever.** The controllers were imported across an ocean to avoid a cloud dependency; the site will not reintroduce one. Every integration talks to hardware on the local network.
2. **Apple Home is never harmed.** iOS users keep their native experience untouched. Every lane below COEXISTS with Apple Home (multiple LAN clients on the same device, or Apple's own stack as the transport) — nothing gets unpaired, nothing migrates.
3. **One binary bias.** Integrations live in the site's Rust process where possible (the d2/weasyprint/ffmpeg shell-out pattern is the accepted escape hatch). Adopting Home Assistant as household infrastructure would solve this whole doc — and is deliberately REJECTED for now as an ecosystem inversion (it becomes the owner of the house; the site becomes a client; the Node/Python service sprawl is what we're trying to shrink, not grow). It stays on the table as the industry answer if the Catalyst lane sours.
4. **Locks don't actuate from a cookie.** A web session — passkey-minted or not — is not the same assurance as FaceID on a phone. Locks and the garage door are READ-ONLY on the panel until a re-auth story exists (WebAuthn re-prompt per actuation is the obvious candidate, its own future slice). Even Apple gates lock actuation behind device auth.

## The Apple wall (research findings, 2026-07)

The obvious idea — "the Rust binary speaks HomeKit" — is walled off twice:

- **No controller-side HAP in reach.** `hap-rs` implements the ACCESSORY side (be a device), not the controller side (drive devices). The mature controller implementation (aiohomekit, what Home Assistant uses) TAKES OVER the accessory — it pairs as the owner, which violates principle 2.
- **The accessory side is BLIND — being on the network buys no read.** HAP's model is strictly controller→accessory: an accessory answers for its OWN characteristics and never learns the home topology (rooms, scenes, the accessory list live in the CONTROLLERS — Apple's synced Home data). A hap-rs bridge can present our devices to Apple Home; it cannot see anyone else's. The most the LAN offers is mDNS `_hap._tcp` advertisements (name/category/paired-flag) — existence, not state. Asked and answered; don't re-litigate.
- **`HMHomeManager` needs an entitlement macOS won't give us.** HomeKit framework access requires a UIKit/Catalyst app carrying the HomeKit entitlement, and on macOS that entitlement only activates for App Store distribution — a Developer ID build launches fine and sees ZERO homes. The workable variant is a DEVELOPMENT-signed Catalyst helper (annual provisioning-profile renewal, accepted as operational cost), and there is direct prior art: HomeClaw (github.com/omarshahine/HomeClaw) is exactly this — a Catalyst app exposing the full home as a CLI/MCP surface.

The crack in the wall, spike-CONFIRMED (2026-07-09): macOS Shortcuts carries the FULL Home action set — **Control Home** (set scenes AND accessories), **Get state** (read accessory state — the readback we assumed didn't exist), and **Toggle Accessory or Scene** — verified in the editor against Duloc. The `shortcuts` CLI answers from a plain ssh session (`shortcuts list` works, no GUI-agent dance), and the app's own runtime context is BETTER than that test: the site runs as a GUI-session LaunchAgent, exactly where Shortcuts wants to live. Expected one-time cost: a TCC consent prompt (Shortcuts → Home data) on first invocation from the app's context — the FDA-grant class of ops step.

## The lanes

**Lane A — Daikin direct (v1, DECIDED).** A `daikin.rs` LAN client speaking the controllers' plain-HTTP API — the same endpoints `homebridge-daikin-local` hits, minus the flaky middleman. Full read/write: current temp, setpoint, mode, fan. Two units, configured by IP in `Settings` (`home.daikin_units: [{name, host}]`). The Homebridge pairing keeps serving Apple Home in parallel; both are just LAN clients of the same controller, last write wins. This is the cheapest real value on the board and it lands the exact ask ("control the A/C").

**Lane B — the estate via `shortcuts run` (v1.5→v2, PROMOTED).** The confirmed action set (Control Home / Get state / Toggle) makes Shortcuts a candidate for per-accessory READ + WRITE across the whole estate, not just scene buttons — Apple's sanctioned automation surface, zero entitlement fight, the d2/ffmpeg shell-out pattern. Build order inside the lane: scene buttons first (dumbest thing that works), then a "Get state" shortcut whose output the Rust side parses for the panel's state cards, then parameterized actuation (`-i` dictionary input → one generic SetAccessory shortcut, or a small family of named ones). Honest costs: shortcut definitions live in the iCloud account and are referenced BY NAME from config (a rename breaks a button — degrade to an error toast, never a 500), a ~1–2s spawn per invocation (fine for a control panel, hopeless for polling — state refreshes on page load + after actions, no background poller), the Get-state OUTPUT SHAPE needs shortcut-side glue to be parseable (the residual spike), and the one-time TCC grant above.

**Lane C — the Catalyst helper (FALLBACK, demoted).** A development-signed Catalyst CLI (HomeClaw-style) exposing the home over a local socket — richer and faster than Lane B (real API, no process spawn, event subscriptions). Reach for it ONLY if Lane B proves too clunky in practice (spawn latency compounds, Get-state parsing stays brittle, or TCC fights back). Costs unchanged: a Swift artifact in the build story + annual profile renewal.

**Lane D — the endgame nicety.** Once the site owns the Daikins directly, Homebridge fronts nothing but dummy switches. `hap-rs` (accessory side — the implementable side) as a coordinator task could expose the site's own integrations INTO Apple Home and retire Homebridge entirely. Small, satisfying, optional — sequenced last because it serves the iOS users who are already fine.

Rejected outright: per-vendor Rust rewrites of the native-HomeKit gear (no vendor APIs exist behind those pairings), HAP-controller takeover (breaks principle 2), Google Cast/speaker tangents (different problem, decided out of this doc).

## The web surface (the DE shape, verbatim)

- Migration seeds a `home` special page row (`'home','/home'`, undeletable) **with `min_role='Family'`** — that one row buys the gated top-level tab (role-aware TopBar), the `/pages/home` redirect and the sign-in gate, exactly like `library`.
- `web/features/home_control.rs` owns the code-defined routes: `/home` (device cards grouped by room) + POST endpoints. Insufficient viewers get the state-aware sign-in gate (`library.rs::gate` generalizes — same copy rules, NO tier names for authenticated-insufficient).
- **The role-scoped mutation allowlist gets its first real entries** (built empty in CZ for exactly this): `POST /home/climate` at `Family`, `POST /home/scene` at `Family`. Device ids ride the request BODY per the allowlist's conventions; per-device checks live in the handler.
- **Every actuation is audited**: `home_actions` table (who, what, when, from-state → to-state where known). Cheap table, high household-trust value; surfaces on the panel for Admin.
- `/home` joins `/library` in the greylist EXACT-path exemptions (a greylisted logged-out family member must reach the sign-in gate).
- `POST /home/*` is excluded from `request_log` if polling/actuation chatter ever pollutes analytics (same self-feed logic as `/challenge`); the panel's own state-refresh GETs likely warrant it from day one.
- **State honesty:** every card carries a last-seen/staleness indicator; a device that doesn't answer renders as UNKNOWN, never as its last happy value. The No Response epidemic is a PERMANENT fact of the estate (enterprise wifi + Thread routers everywhere didn't fix it), so the panel reports it instead of laundering it.
- **Device-health log (the observability Apple won't give):** every Lane B state fetch also writes `device_health` (device, responded?, ts) — cheap rows at page-load rates — feeding a flaky-device leaderboard on the panel: last-answered timestamps, flap counts, "the front door lock has been dead since June 18". Same measure→surface→retune instinct as the analytics dashboard and the greylist evidence panel; Apple's own app buries a three-week outage in a grey tile.

## Environment split — beta must never actuate the house

Both instances run ON THE SAME MINI, so a beta deploy with home routes would control the real house through the same LAN. The gate is config, not code: the `home` Settings block (Daikin IPs, scene names) simply DOESN'T EXIST in beta's config file — absent config → the tab renders a "not configured" state and every POST 404s. No `debug_assertions` seam, no scrub step to forget; beta stays safe by construction and the beta DB snapshot carries nothing actuation-relevant.

## CSRF + session notes

The session cookie is `SameSite=Lax`, so cross-site POSTs don't carry it — the actuation endpoints inherit CSRF protection from the existing cookie posture (no token machinery needed). The 7-day greylist clearance and 1-day rolling session expiry both already assume a phone that wanders between networks; nothing here changes session semantics.

## Phasing (DH, on ratification)

- **DH.1** — `home` special row migration + `/home` tab + gate + empty room-grouped panel shell (the DE checklist replayed).
- **DH.2** — `daikin.rs` LAN client (probe/status/set; unit tests against canned controller responses) + Settings block + climate cards with live state + setpoint/mode control via the allowlist.
- **DH.3** — audit table + panel staleness treatment + request_log exclusions.
- **DH.4** — Lane B build-out: scene buttons via `shortcuts run` (+ the one-time TCC grant from the app's LaunchAgent context), then the Get-state output-parsing spike → estate state cards + parameterized actuation if the shape holds.
- **DH.5** — tests (gate oracle parity with library, allowlist entries, Daikin client KATs) + CLAUDE.md delta + beta not-configured verification + prod tag.
- **v2/v3 (unscheduled)** — Catalyst helper ONLY if Lane B proves clunky; hap-rs bridge retiring Homebridge; lock actuation behind WebAuthn re-prompt.

## Deferred / decided out

Voice assistants as a BUILD target (different integration universe) — but note the existing HomePods already give non-iOS family Siri control of the native gear TODAY, zero code (secure accessories correctly refuse unrecognized voices). More-hubs-fixes-flakiness is REFUTED by lived data: this house already has enterprise wifi + Thread border routers everywhere and devices still wander off — the No Response epidemic is intrinsic smart-home instability, which is exactly why the staleness-honesty principle and the device-health log below exist. Audio casting (separate chew, separate doc if ever), Home Assistant adoption (rejected-for-now above), lock/garage actuation (read-only until re-auth exists), Matter multi-admin experiments (revisit only if Lane C sours), live device discovery (the Daikin units are two static IPs on a home LAN — config is fine).
