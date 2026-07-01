//! Process-lifetime, content-addressed cache for the two EXPENSIVE pure markdown
//! renders — the full HTML [`transform`] and the [`excerpt`]. Both are
//! deterministic functions of their input markdown (neither reads the request,
//! host, or `AppState`), so the CONTENT is a safe cache key: a page edit changes
//! the markdown → a new key → a miss → a fresh render, and the stale entry is
//! simply never hit again. Invalidation is therefore FREE — no write-path wiring,
//! exactly the trick the d2 diagram cache (`diagram.rs`) already plays.
//!
//! WHY IN-MEMORY (not persisted to disk/DB): `transform` is not side-effect-free
//! — it registers each ` ```d2 ` block's source into the process-lifetime diagram
//! `REGISTRY` that `/diagram/<hash>` later reads to render the SVG. A cache HIT
//! skips `transform` and therefore skips that registration. This stays correct
//! ONLY because this cache and the registry share the SAME lifetime (process
//! memory, both empty after a restart): a hit can only occur AFTER a miss ran
//! `transform` this process, which registered that content's diagrams. A
//! disk-persisted cache would break this — it would survive a restart that
//! emptied the registry, then serve HTML whose `/diagram/<hash>` lookups 404.
//!
//! MOTIVATION (Phase CS): `/feed.xml` transformed EVERY entry (up to ~100 pages)
//! on every request — ~1.3s, the slowest route on the site — and every page view
//! re-transformed its own body. With this, transform runs once per DISTINCT
//! content across the feed AND all page renders.
//!
//! No eviction / size cap — matches the diagram + résumé-PDF caches. The corpus
//! is a personal site (dozens of pages) and the process restarts on every deploy,
//! so growth is bounded by "distinct content rendered between restarts". If that
//! ever matters, an LRU cap is the lever (same as those two caches).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use anyhow::Result;
use openssl::sha::sha256;

use crate::web::markdown::excerpt::excerpt;
use crate::web::markdown::transformer::transform;

/// hash(markdown) -> rendered HTML.
static TRANSFORM_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// hash(markdown) -> excerpt plaintext.
static EXCERPT_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Content hash of the input: SHA-256 truncated to 128 bits, hex. Matches the
/// keying in `diagram.rs` / `resume.rs` so the whole codebase caches the same way.
fn content_hash(source: &str) -> String {
    let digest = sha256(source.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Cached [`transform`]. Content-hash keyed → automatic invalidation on edit. On a
/// MISS the transform runs, registering any ` ```d2 ` diagrams into the process
/// registry (see module docs on why that stays coherent with an in-memory cache).
/// Only successful renders are cached — an `Err` is deterministic for its input
/// but rare, and not caching it mirrors the diagram cache's error handling.
pub fn cached_transform(markdown: &str) -> Result<String> {
    let key = content_hash(markdown);
    if let Some(hit) = TRANSFORM_CACHE
        .lock()
        .expect("transform cache poisoned")
        .get(&key)
    {
        return Ok(hit.clone());
    }
    let html = transform(markdown)?;
    TRANSFORM_CACHE
        .lock()
        .expect("transform cache poisoned")
        .insert(key, html.clone());
    Ok(html)
}

/// Cached [`excerpt`]. Same content-hash keying as [`cached_transform`].
pub fn cached_excerpt(markdown: &str) -> String {
    let key = content_hash(markdown);
    if let Some(hit) = EXCERPT_CACHE
        .lock()
        .expect("excerpt cache poisoned")
        .get(&key)
    {
        return hit.clone();
    }
    let ex = excerpt(markdown);
    EXCERPT_CACHE
        .lock()
        .expect("excerpt cache poisoned")
        .insert(key, ex.clone());
    ex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::markdown::diagram;

    #[test]
    fn content_hash_is_stable_and_distinct() {
        assert_eq!(content_hash("a body"), content_hash("a body"));
        assert_ne!(content_hash("a body"), content_hash("b body"));
        assert_eq!(content_hash("a body").len(), 32, "128-bit hex");
    }

    #[test]
    fn cached_transform_matches_uncached_and_is_deterministic() {
        let md = "# Title\n\nSome **bold** prose and a [link](/x).";
        let direct = transform(md).unwrap();
        let first = cached_transform(md).unwrap();
        let second = cached_transform(md).unwrap();
        assert_eq!(direct, first, "cache must not change the output");
        assert_eq!(first, second, "repeat calls are stable");
    }

    #[test]
    fn cached_excerpt_matches_uncached() {
        let md = "First paragraph.\n\nSecond.";
        assert_eq!(cached_excerpt(md), excerpt(md));
        // hit path returns the same value
        assert_eq!(cached_excerpt(md), "First paragraph.");
    }

    #[test]
    fn cached_transform_still_registers_diagrams() {
        // The coherence guarantee: routing transform THROUGH the cache must still
        // populate the diagram registry on the miss, or `/diagram/<hash>` 404s.
        // Use a body unique to this test so its content hash can't be a warm hit
        // from another test (the caches are process-global).
        let md = "before\n\n```d2\ncs_cache_probe_a -> cs_cache_probe_b\n```\n\nafter";
        let html = cached_transform(md).unwrap();
        // Pull the registered hash out of the emitted `hx-get="/diagram/<hash>"`.
        let marker = "/diagram/";
        let start = html.find(marker).expect("d2 block must emit a diagram swap") + marker.len();
        let end = start + html[start..].find('"').expect("hx-get value must close");
        let hash = &html[start..end];
        assert!(
            diagram::render_registered(hash).is_some(),
            "cached_transform must register the diagram source (hash {hash} not in registry)"
        );
    }
}
