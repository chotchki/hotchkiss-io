# Plan

Completed phases are in `PLAN_ARCHIVE.md` (most recent: Phase 0 — push-to-deploy on the Mac mini; Phase 4 — `tray-wrapper` 0.4.1 bump).

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

## Phase 3 — Replace `ifconfig.me` with Cloudflare `cdn-cgi/trace` (unblocked — Phase 0 landed)

**Motivation:** `ifconfig.me` is an external service that may go silently down; we already trust Cloudflare for DNS, so collapsing public-IP discovery into Cloudflare introduces no *new* dependency. `https://1.1.1.1/cdn-cgi/trace` returns `key=value\n` lines including `ip=<addr>`. Connecting to the IPv4 literal `1.1.1.1` forces an IPv4 path, which matches current behavior (`Ipv4Addr` only). Also kills the one transient test flake (`ifconfig::tests::basic_run` occasionally fails on a network blip).

Current code: `src/coordinator/ip/ifconfig.rs` defines `IfconfigMe::public_ip() -> Result<Ipv4Addr>`; `src/coordinator/ip_provider_service.rs` is the only caller.

- [x] 3.1 Added `src/coordinator/ip/cloudflare_trace.rs` — `CloudflareTrace::new()` (rustls reqwest client) + `public_ip() -> Result<Ipv4Addr>` (GET `https://1.1.1.1/cdn-cgi/trace`, `error_for_status`, then `parse_ip` splits on lines, `strip_prefix("ip=")`, parses `Ipv4Addr`). `parse_ip` is a private associated fn so it's unit-testable without a network round-trip; it `.context()`s a clear error if the `ip=` line is missing or the value won't parse.
- [x] 3.2 Unit tests: `parses_ip_from_sample` (full captured key=value block → `203.0.113.42`), `missing_ip_line_errors`, `malformed_ip_value_errors`.
- [x] 3.3 `#[tokio::test] async fn basic_run` hits the live `1.1.1.1/cdn-cgi/trace` and asserts `!addr.is_private()` — mirrors the old `ifconfig::tests::basic_run`.
- [x] 3.4 `IpProviderService` now holds a `CloudflareTrace`; import changed to `super::ip::cloudflare_trace::CloudflareTrace`. `server_ips()` body unchanged (still `self.client.public_ip().await?`).
- [x] 3.5 Deleted `src/coordinator/ip/ifconfig.rs`; `src/coordinator/ip/mod.rs` is now just `pub mod cloudflare_trace;`.
- [x] 3.6 CLAUDE.md "Runtime architecture" bullet updated (`IpProviderService` polls `1.1.1.1/cdn-cgi/trace`, parses `ip=`, IPv4 literal forces v4). SPEC.md "Self contained" list: dropped the `ifconfig.me` line, folded IP discovery into the existing Cloudflare dependency.
- [x] 3.7 `cargo build` + `cargo clippy --all-targets` clean (only the pre-existing warnings: dead `update` methods, `page_path`, collapsible match, `Context` import in `service_coordinator.rs`). `cargo test`: 22/22 (was 19; +3 net — three new unit tests, and `cloudflare_trace::tests::basic_run` replaces `ifconfig::tests::basic_run`). The live `basic_run` passed, which also retires the old transient `ifconfig.me` flake.
- [ ] 3.8 Manual e2e: after `git push origin main`, confirm the new code reads the public IP without error — check the launchd stderr log on the mini for "Sending Initial IP addresses {...}" with the real address (and no "Had an error getting the ip address"), and that `dig hotchkiss.io` still matches `ssh hotchkiss.io 'curl -s https://1.1.1.1/cdn-cgi/trace | grep ^ip='`. (Debug builds short-circuit to `127.0.0.1` — untouched.)

## Phase 5 — Drop the patched `cookie` fork

**Motivation:** Cookie 0.18.x still doesn't ship serde impls upstream (confirmed 2026-05-09). We currently maintain a fork (`chotchki/cookie-rs` `serde_support` branch) wired in via `[patch.crates-io]` in `Cargo.toml`. CLAUDE.md explicitly calls out the patch as a watch-out. Maintaining a fork to add a couple of trait impls is much heavier than serde's remote-derive pattern (https://serde.rs/remote-derive.html), which lets us provide `Serialize`/`Deserialize` for `cookie::Cookie` from our own crate without forking.

**Discovery:** the working tree only references `tower_sessions::cookie::Key` (the session-signing-key newtype) and never directly serializes `cookie::Cookie`. The patch may be dead — needed by a transitive dep that has since dropped the requirement. Check before assuming we need the workaround.

- [ ] 5.1 Try the no-op path first: comment out the `[patch.crates-io]` block in `Cargo.toml`, `cargo update -p cookie`, `cargo build`. If it builds, the patch was dead code — proceed to 5.5.
- [ ] 5.2 If 5.1 fails, locate the transitive consumer that wants `Cookie: Serialize/Deserialize` (`cargo tree -i cookie -e features` and read the build error). That tells us which crate's API forces the requirement.
- [ ] 5.3 Add `src/cookie_remote.rs` (or similar) with a `CookieDef` newtype, `#[serde(remote = "cookie::Cookie")]`, mirroring the public-field shape of `cookie::Cookie`. Annotate the consumer's call sites with `#[serde(with = "cookie_remote::CookieDef")]`.
- [ ] 5.4 If the transitive consumer is itself defining serde structs around `Cookie` (i.e. we can't reach the call site), the remote-derive escape hatch doesn't apply — at that point either the upstream crate needs a feature flag or we keep the fork. Document the finding and revert.
- [ ] 5.5 With the patch removed, drop `[patch.crates-io]` entirely from `Cargo.toml`, the corresponding lockfile entries, and the CLAUDE.md "Patched `cookie` crate" caveat.
- [ ] 5.6 `cargo build` + `cargo clippy --all-targets` clean; `cargo test` 19/19 passing.
