# Plan

Completed phases are in `PLAN_ARCHIVE.md` (most recent: Phase 5 — dropped the `cookie-rs` fork; Phase 3 — `ifconfig.me` → Cloudflare `cdn-cgi/trace`; Phase 0 — push-to-deploy on the Mac mini; Phase 4 — `tray-wrapper` 0.4.1 bump).

## Phase 1 — Fix `get_recs_by_name` hardcoded `type=A` filter

**Symptom:** ACME cert renewal hangs forever in `DnsValidator::ensure_not_existing` polling for a stale `_acme-challenge` TXT record that never disappears.

**Root cause:** `src/coordinator/dns/cloudflare_api.rs:146` pins the Cloudflare query to `type=A`. When `clean_proof` calls `get_recs_by_name` for the `_acme-challenge` domain, Cloudflare returns 0 results (no A records exist there), the delete loop is a no-op, and no TXT records are ever removed. `ensure_not_existing` then polls indefinitely.

- [x] 1.1 Add a record-type parameter to `CloudflareApi::get_recs_by_name` (`rec_type: &str`) and use it in the query string.
- [x] 1.2 Update `clean_proof` (`cloudflare_client.rs`) to pass `"TXT"`.
- [x] 1.3 Update `update_dns` (`cloudflare_client.rs`) to pass `"A"` (preserves current behavior; keeps `Ipv4Addr::from_str(&rec.content)` parsing safe).
- [x] 1.4 `cargo build` + `cargo clippy` clean (only pre-existing warnings remain).
- [x] 1.5 `cargo test` passes (18/18).
- [ ] 1.6 Manual e2e: trigger an ACME renewal in prod (or shorten the renewal window in dev) and confirm `clean_proof` deletes leftover TXT records before `create_proof` recreates them. *No automated e2e exists for the ACME path — this gap is tracked in Phase 2.* Note: the fix is live in production as of the Phase 0 deploys (the binary running on the mini includes it), so this is "watch the next real renewal" rather than "deploy and test".
- [ ] 1.7 Docs: no CLAUDE.md changes needed (behavior fix, no architectural shift). Confirm and tick.

## Phase 2 — DNS module testability (deferred, tracked)

The DNS module has zero tests today. The bug in Phase 1 would have been caught by a unit test on `get_recs_by_name`'s URL construction. Worth fixing but out of scope for the immediate hotfix.

- [ ] 2.1 Extract URL-building from `CloudflareApi` methods into pure helpers.
- [ ] 2.2 Add unit tests covering: query string includes name + type for each call site; type is not hardcoded.
- [ ] 2.3 Decide on HTTP mocking strategy (`wiremock`, `mockito`, hand-rolled) for higher-level tests of `clean_proof` / `create_proof` / `update_dns`.
- [ ] 2.4 Add tests for `DnsValidator::ensure_exists` and `ensure_not_existing` that don't hit a real resolver (would have surfaced the infinite-loop behavior earlier).

