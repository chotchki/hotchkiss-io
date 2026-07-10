# MCP dogfood feedback (first real agent-driven post)

Ranked feedback from chris's OTHER project driving the live `/mcp` (v1.5.1) to publish a post — captured as Phase DK. This is the WHY + the priorities; PLAN.md carries the tasks.

**Scope:** polish the MCP server's ergonomics + error semantics from REAL usage — actionable errors, strict inputs, a deliberate auth response. NOT re-architecting: the transport + tool surface (DI) held up. The positives in item 4 are load-bearing — don't regress them.

## The feedback, ranked

1. **Duplicate title leaks the raw SQLite error (highest).** A second page with the same slug under a parent returns `-32603 "UNIQUE constraint failed: content_pages.parent_page_id, content_pages.page_name"` — internal SCHEMA names cross the API boundary and the message isn't actionable. Fix: catch the unique-constraint violation in `create_page` and return `-32602` (invalid params) `"a page with this slug already exists under <parent>"`. chris's contrast is the tell: the delete confirm-gate message is exactly the right shape — actionable, no internals. (Info-leak severity is low — `/mcp` is Admin-only, so the "attacker" is already an authenticated admin — but the UX is bad and the boundary leak is sloppy.)

2. **Unknown argument fields are silently ignored.** A typo'd `list_pages {path: "blog"}` (the field is `parent_path`) returned a SUCCESSFUL WRONG answer (the top-level listing) instead of an error. A typo'd key must be a HARD error. Fix: `#[serde(deny_unknown_fields)]` on every MCP param struct. **This folds into DJ.5** — the wire-struct retyping touches exactly these structs, so `deny_unknown_fields` lands there, not as a separate DK task.

3. **No-auth returns 403 with no body.** `401 + WWW-Authenticate` is the more accurate semantic for MISSING credentials (403 = "authenticated but forbidden"). Two REAL tensions to resolve deliberately, NOT a blind flip:
   - **`WWW-Authenticate` triggers Claude Code's OAuth discovery** (DI design doc: Claude Code chases OAuth on a 401/403 that carries `WWW-Authenticate`). chris's client was plain curl so it didn't hit this; a Claude Code client would. So the accurate move is `401` WITHOUT `WWW-Authenticate` — semantic win, no OAuth chase.
   - **The 403 comes from the ONE global `require_admin_for_mutations` layer** (chris's own "one authz path" rule). Making `/mcp` alone return 401 diverges from that. Cleaner reconciliation: the GLOBAL layer distinguishes a MISSING identity (`Anonymous` → 401) from an INSUFFICIENT one (authenticated non-admin → 403), site-wide — a semantic improvement everywhere, not a `/mcp` special case. Decide this deliberately.

4. **Validated positives — do NOT regress.** Stateless (no `Mcp-Session-Id`) worked flawlessly with a plain-curl client (right call for this use case); `get_page` on a bogus path returns a clean spec-correct `-32002 page not found`; slug derivation (`yes-i-ll-...`) is consistent with the existing `let-s-...` convention. These are the DI choices paying off.

## Phasing (DK)

- **DK.1** — duplicate-slug create → `-32602` actionable message (`"a page with this slug already exists under <parent>"`); catch the UNIQUE violation, never leak the `content_pages.*` schema. A test.
- **DK.2** — auth response semantics: at the ONE global layer, `401` for a MISSING identity vs `403` for an insufficient one (site-wide, not `/mcp`-special), WITHOUT `WWW-Authenticate` (no OAuth chase). A test.
- (item 2, `deny_unknown_fields`, folds into DJ.5.)

## Resolution (shipped v1.5.2)

- **DK.1 ✓** — `PageWrite::create_page` downcasts the create error to a `sqlx` UNIQUE violation (`is_unique_violation`) → `PageWriteError::DuplicateSlug{slug,parent}`, mapped to `-32602` "a page with slug '…' already exists under <parent>" over MCP and a **409** over the editor HTTP path. The raw `content_pages` constraint text never crosses the boundary. Tests: `duplicate_slug_create_is_actionable_not_a_leaked_constraint` (mcp), `duplicate_slug_create_returns_409_over_http` (web).
- **DK.2 ✓** — the global `require_admin_for_mutations` + the `/admin` `require_admin` layer both split the deny: Anonymous (missing) → `401` `unauthorized_response` ("Who goes there?"), authenticated-insufficient → `403` `forbidden_response` ("How about NO!"). The 401 carries NO `WWW-Authenticate` (chosen deliberately per item 3 — no OAuth chase, no basic-auth popup). Site-wide, one path. Tests: `auth_split_401_missing_403_insufficient_no_www_authenticate` (+ the styled-page / htmx-redirect / e2e coverage split into 401-vs-403 variants).
- **item 2 ✓** — `deny_unknown_fields` shipped in DJ.5 (`a_typoed_argument_key_is_a_hard_error`).

### Re-probe follow-ups (shipped v1.5.3)

- **DK.3 ✓ — serverInfo identity.** On re-probe the server still reported `rmcp 2.2.0` — the SDK default (`Implementation::from_build_env()` resolves at RMCP's OWN compile time, so every un-set rmcp server looks identical in client logs/UIs). `get_info` now stamps `server_info.name = "hotchkiss-io"`, `version = CARGO_PKG_VERSION`, `title = "hotchkiss.io publishing"`. The round-trip test pins our name+version on the initialize response and asserts `rmcp` doesn't leak.
- **401 no `WWW-Authenticate` — confirmed intentional, no change.** The re-probe nit ("only matters if you ever want OAuth-style resource discovery, ignorable for a personal bearer-token server") agrees with the DK.2 decision: the 401 deliberately carries no challenge header (no OAuth chase, no basic-auth popup). Left as-is.
