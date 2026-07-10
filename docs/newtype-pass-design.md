# Newtype pass across the content model

Design note for Phase DJ — a typing pass that replaces the string-and-i64 primitives carrying domain meaning (visibility, ids, slugs, media tokens, paths) with newtypes. This is the single source for WHY the pass is shaped this way; the code carries the how, PLAN.md the task breakdown.

**Scope:** this is a REFACTOR, not a feature — no behavior change, no new surface. The goal is strong typing as testing-before-the-test: make the compiler reject a slug used as a path, a page_id passed where a media_id belongs, or a raw `min_role` string that skipped the fail-closed decode. What's NOT the job: reworking the storage schema (the columns stay TEXT / INTEGER), changing the wire format (MCP + forms stay strings on the wire via `#[serde(transparent)]`), or chasing every last primitive — only the ones that carry domain meaning and get mixed up or duplicated.

## The problem, concretely

The content model passes domain values as bare `String` / `i64`, and the meaning lives in the variable name, not the type. Two concrete costs:

- **Duplicated fail-closed decode.** `min_role` is `Option<String>` (NULL = public), and the fail-closed rank decode (`None→0 / Registered→1 / Family→2 / else→Admin`) is written TWICE — once in `content_pages` (`min_role_rank` + the SQL CASE) and once in `media` (`MediaDao::min_role_rank` + its CASE). Two copies of a security-critical decode is exactly the drift the CLAUDE.md keeps warning about, and DI added a THIRD string-match of the same policy in the MCP structs + the web `PutPageForm`. This is the highest-leverage newtype: one `MinRole` type owning one decode.
- **Interchangeable primitives.** `page_id` and `media_id` are both `i64`; `page_name` (slug), a `media_ref`, a `url_key`, and a `/`-joined page path are all `String`. Nothing stops one being passed where another belongs — a class of bug the type system should own, not code review.

## The inventory (ROI order)

- **`MinRole` / `Visibility`** — `≈ Option<Role>` with ONE fail-closed decode + ONE SQL-CASE emitter, replacing the duplicated `min_role_rank`. Threads through `PageUpdate`, the MCP create/update structs, the web `PutPageForm`, and both DAOs. The security payoff (single decode) makes this the anchor of the pass.
- **`PageId` / `MediaId`** — `i64` newtypes. Cheap, and they kill id-mixups across the DAO surface + call sites.
- **`Slug`** — the validated `page_name` (`slugify` returns a `Slug`, not a `String`); non-empty by construction.
- **`MediaRef` / `UrlKey`** — the two media tokens (the opaque author ref vs the HMAC byte-serve key), reconciled with the existing `MediaReference` enum so there's one parse path.
- **`PagePath`** — the resolved tree path (`find_by_path` takes a `PagePath`, not `&[&str]`).

## What this deliberately does NOT do

- **No schema change.** Columns stay TEXT / INTEGER; the newtypes are `sqlx`-encode/decode wrappers, not new tables.
- **No wire-format change.** MCP tool params/results + HTML form fields stay strings/ints on the wire — the newtypes are `#[serde(transparent)]` with a VALIDATING `Deserialize` (a bad slug/ref/role is rejected at the boundary, which is a bonus, not a break).
- **Not every primitive.** A `title` stays a `String` (it's free text, nothing to confuse it with); a `page_order` stays `i64`. Only domain values that get mixed up or carry a duplicated decode.
- **No `Role`-reshaping.** `Role` + `rank()` stay as they are (the ladder is already pinned); `MinRole` wraps `Option<Role>`, it doesn't replace the enum.

## Phasing (DJ)

One task per type family, each threaded end to end (DAO → service → boundary) so the type actually flows rather than being unwrapped at the edge. `MinRole` first (it centralizes the security-critical decode and unblocks the rest); the MCP + web boundaries retype last, once the interior types exist. The DI structs written with strings get retyped in DJ.5 — deliberately deferred from DI so the pass is uniform, not piecemeal.
