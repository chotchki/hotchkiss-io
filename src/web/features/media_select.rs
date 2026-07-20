//! Variant selection + content negotiation for the media resource (Phase DP).
//!
//! Pure functions over a variant list — the ONE selector shared by the negotiated
//! `GET /media/<ref>` (§5), the `OPTIONS` manifest, and the embed's per-kind picks.
//! See `docs/media-design.md` §5.
//!
//! **Precedence:** `?format=` (an explicit DEMAND — match or 406) > a wildcard-free
//! `Accept` (a genuine preference) > largest (the default). A wildcard-bearing
//! `Accept` (every browser sends `…,*/*`) is treated as "no preference" → largest,
//! so a plain download link / `fab publish` link is UNCHANGED — only a deliberate
//! `?format=` or a `*/*`-free `Accept` selects a specific representation.

use crate::db::dao::media::{MediaVariantDao, ModelFormat};

/// Map a `?format=<token>` to the mime it selects. The vocabulary is small +
/// server-defined; the manifest advertises concrete `href`s so a client never has
/// to hardcode it. `None` → an unknown token (→ 406 at the route).
pub fn format_token_to_mime(token: &str) -> Option<&'static str> {
    match token.trim().to_ascii_lowercase().as_str() {
        "scad" => Some("application/x-openscad"),
        "stl" => Some("model/stl"),
        "3mf" => Some("model/3mf"),
        "avif" => Some("image/avif"),
        "webp" => Some("image/webp"),
        "png" => Some("image/png"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "mp4" => Some("video/mp4"),
        "mp3" => Some("audio/mpeg"),
        "m4a" | "m4b" | "aac" => Some("audio/mp4"),
        "flac" => Some("audio/flac"),
        _ => None,
    }
}

/// The mime "essence" — type/subtype, lowercased, parameters dropped.
fn essence(mime: &str) -> &str {
    mime.split(';').next().unwrap_or("").trim()
}

fn mime_eq(a: &str, b: &str) -> bool {
    essence(a).eq_ignore_ascii_case(essence(b))
}

/// The largest variant overall (the download / negotiation default).
pub fn largest(variants: &[MediaVariantDao]) -> Option<&MediaVariantDao> {
    variants.iter().max_by_key(|v| v.bytes)
}

/// The largest variant whose mime equals `mime` (format selection).
pub fn largest_of_mime<'a>(
    variants: &'a [MediaVariantDao],
    mime: &str,
) -> Option<&'a MediaVariantDao> {
    variants.iter().filter(|v| mime_eq(&v.mime, mime)).max_by_key(|v| v.bytes)
}

/// The largest renderable/printable MESH (STL or 3MF) — the model DOWNLOAD pick.
pub fn largest_mesh(variants: &[MediaVariantDao]) -> Option<&MediaVariantDao> {
    variants
        .iter()
        .filter(|v| ModelFormat::from_mime(essence(&v.mime)).is_some_and(ModelFormat::is_mesh))
        .max_by_key(|v| v.bytes)
}

/// The viewer MESH: the smallest `model/3mf` (color + fast), else the smallest mesh
/// of any kind. The three.js viewer's pick (color preferred, small for a fast fetch).
pub fn viewer_mesh(variants: &[MediaVariantDao]) -> Option<&MediaVariantDao> {
    variants
        .iter()
        .filter(|v| mime_eq(&v.mime, "model/3mf"))
        .min_by_key(|v| v.bytes)
        .or_else(|| {
            variants
                .iter()
                .filter(|v| ModelFormat::from_mime(essence(&v.mime)).is_some_and(ModelFormat::is_mesh))
                .min_by_key(|v| v.bytes)
        })
}

/// Image variants carrying a pixel WIDTH, sorted ASCENDING — the srcset ladder (§8).
/// A legacy image with no widths yields an empty ladder, so the embed falls back to a
/// single `src`. The largest is the src (best for a no-srcset client + the zoom view).
pub fn image_ladder(variants: &[MediaVariantDao]) -> Vec<&MediaVariantDao> {
    let mut sized: Vec<&MediaVariantDao> = variants
        .iter()
        .filter(|v| is_web_displayable_image(&v.mime) && v.width.is_some())
        .collect();
    sized.sort_by_key(|v| v.width.unwrap_or(0));
    sized
}

/// Browser-displayable image mime. HEIC/HEIF are STORED (the original bytes stay
/// source-of-truth) but never picked for an embed/cover ladder — only Safari
/// renders them; browsers get the ingest-derived AVIF rungs instead (EB.9).
pub fn is_web_displayable_image(mime: &str) -> bool {
    mime.starts_with("image/") && !matches!(mime, "image/heic" | "image/heif")
}

/// A parsed `Accept` media range.
struct AcceptRange {
    ty: String,
    sub: String,
    q: f32,
}

impl AcceptRange {
    fn is_total_wildcard(&self) -> bool {
        self.ty == "*" && self.sub == "*"
    }
    /// Does `mime` match this range with q > 0? (`*/*`, `type/*`, or exact.)
    fn accepts(&self, mime: &str) -> bool {
        if self.q <= 0.0 {
            return false;
        }
        let Some((ty, sub)) = essence(mime).split_once('/') else {
            return false;
        };
        (self.ty == "*" || self.ty.eq_ignore_ascii_case(ty))
            && (self.sub == "*" || self.sub.eq_ignore_ascii_case(sub))
    }
}

fn parse_accept(header: &str) -> Vec<AcceptRange> {
    header
        .split(',')
        .filter_map(|part| {
            let mut it = part.split(';');
            let mime = it.next()?.trim();
            let (ty, sub) = mime.split_once('/')?;
            let mut q = 1.0f32;
            for param in it {
                if let Some(v) = param.trim().strip_prefix("q=") {
                    q = v.trim().parse().unwrap_or(1.0);
                }
            }
            Some(AcceptRange {
                ty: ty.trim().to_ascii_lowercase(),
                sub: sub.trim().to_ascii_lowercase(),
                q,
            })
        })
        .collect()
}

/// The outcome of GET-negotiation over a variant list.
#[derive(Debug, PartialEq)]
pub enum Negotiation<'a> {
    /// Redirect to this variant's bytes.
    Variant(&'a MediaVariantDao),
    /// Return the item-state JSON (the caller explicitly asked for `application/json`).
    ItemState,
    /// The item has no variants at all → 404.
    Empty,
    /// An explicit `?format=` / wildcard-free `Accept` the item can't satisfy → 406.
    NotAcceptable,
}

/// Negotiate `GET /media/<ref>`. See the module + `docs/media-design.md` §5.
pub fn negotiate<'a>(
    variants: &'a [MediaVariantDao],
    format: Option<&str>,
    accept: Option<&str>,
) -> Negotiation<'a> {
    // `?format=` is a DEMAND: match the largest of that mime, or 406 — never fall through.
    if let Some(fmt) = format.map(str::trim).filter(|s| !s.is_empty()) {
        return format_token_to_mime(fmt)
            .and_then(|m| largest_of_mime(variants, m))
            .map_or(Negotiation::NotAcceptable, Negotiation::Variant);
    }

    let ranges = parse_accept(accept.unwrap_or(""));
    // A wildcard-bearing (or absent) Accept = "no preference" → the default download.
    // A browser always lands here (it sends `…,*/*`), so bare-link behavior is unchanged.
    let has_wildcard =
        accept.is_none_or(str::is_empty) || ranges.iter().any(|r| r.q > 0.0 && r.is_total_wildcard());
    if has_wildcard {
        return largest(variants).map_or(Negotiation::Empty, Negotiation::Variant);
    }

    // A wildcard-FREE Accept is a genuine preference.
    if ranges
        .iter()
        .any(|r| r.q > 0.0 && r.ty == "application" && r.sub == "json")
    {
        return Negotiation::ItemState;
    }
    variants
        .iter()
        .filter(|v| ranges.iter().any(|r| r.accepts(&v.mime)))
        .max_by_key(|v| v.bytes)
        .map_or(Negotiation::NotAcceptable, Negotiation::Variant)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(url_key: &str, mime: &str, bytes: i64) -> MediaVariantDao {
        MediaVariantDao {
            variant_id: 1,
            media_id: 1,
            sha256: "sha".into(),
            url_key: url_key.into(),
            mime: mime.into(),
            codecs: None,
            bytes,
            storage_root: None,
            width: None,
            height: None,
        }
    }

    fn picked<'a>(n: Negotiation<'a>) -> &'a str {
        match n {
            Negotiation::Variant(x) => &x.url_key,
            other => panic!("expected a variant, got {other:?}"),
        }
    }

    #[test]
    fn format_token_maps_the_vocabulary() {
        assert_eq!(format_token_to_mime("scad"), Some("application/x-openscad"));
        assert_eq!(format_token_to_mime("3MF"), Some("model/3mf"));
        assert_eq!(format_token_to_mime("m4b"), Some("audio/mp4"));
        assert_eq!(format_token_to_mime("nope"), None);
    }

    #[test]
    fn format_is_a_demand_largest_of_mime_or_406() {
        let vs = [v("scad", "application/x-openscad", 100), v("low", "model/3mf", 50), v("high", "model/3mf", 900)];
        assert_eq!(picked(negotiate(&vs, Some("scad"), None)), "scad");
        assert_eq!(picked(negotiate(&vs, Some("3mf"), None)), "high", "largest of that mime");
        assert_eq!(negotiate(&vs, Some("mp4"), None), Negotiation::NotAcceptable, "no such mime → 406");
        assert_eq!(negotiate(&vs, Some("bogus"), None), Negotiation::NotAcceptable, "unknown token → 406");
    }

    #[test]
    fn a_browser_wildcard_accept_gets_the_largest_unchanged() {
        // A png-original image + its avif ladder. A browser (…,image/avif,…,*/*) must
        // still get the largest overall (the original), NOT the largest avif — bare-link
        // download behavior is preserved because the Accept carries `*/*`.
        let vs = [v("orig", "image/png", 5000), v("w480", "image/avif", 800), v("w960", "image/avif", 2000)];
        let browser = "text/html,application/xhtml+xml,image/avif,image/webp,*/*;q=0.8";
        assert_eq!(picked(negotiate(&vs, None, Some(browser))), "orig");
        assert_eq!(picked(negotiate(&vs, None, None)), "orig", "absent Accept → largest");
        assert_eq!(picked(negotiate(&vs, None, Some("*/*"))), "orig");
    }

    #[test]
    fn a_wildcard_free_accept_is_an_honest_preference() {
        let vs = [v("orig", "image/png", 5000), v("w480", "image/avif", 800), v("w960", "image/avif", 2000)];
        assert_eq!(picked(negotiate(&vs, None, Some("image/avif"))), "w960", "largest acceptable avif");
        assert_eq!(picked(negotiate(&vs, None, Some("image/*"))), "orig", "image/* accepts the png too → largest");
        assert_eq!(
            negotiate(&vs, None, Some("video/mp4")),
            Negotiation::NotAcceptable,
            "specific + unsatisfiable → 406"
        );
        assert_eq!(negotiate(&vs, None, Some("application/json")), Negotiation::ItemState);
    }

    #[test]
    fn empty_item_negotiates_to_empty() {
        assert_eq!(negotiate(&[], None, None), Negotiation::Empty);
        assert_eq!(negotiate(&[], Some("scad"), None), Negotiation::NotAcceptable);
    }

    #[test]
    fn image_ladder_is_width_sorted_and_image_only() {
        let mut orig = v("orig", "image/avif", 5000);
        orig.width = Some(1600);
        let mut w960 = v("w960", "image/avif", 2000);
        w960.width = Some(960);
        let mut w480 = v("w480", "image/avif", 800);
        w480.width = Some(480);
        let mut legacy = v("legacy", "image/png", 100); // no width → excluded
        legacy.width = None;
        let poster = v("poster", "video/mp4", 9); // not an image → excluded
        let vs = [orig, w960, w480, legacy, poster];
        let ladder = image_ladder(&vs);
        let keys: Vec<&str> = ladder.iter().map(|v| v.url_key.as_str()).collect();
        assert_eq!(keys, vec!["w480", "w960", "orig"], "ascending width, images-with-width only");
    }

    #[test]
    fn mesh_helpers_pick_by_type_and_size() {
        let vs = [
            v("img", "image/avif", 20),
            v("scad", "application/x-openscad", 100),
            v("low", "model/3mf", 120),
            v("high", "model/3mf", 2400),
        ];
        assert_eq!(largest_mesh(&vs).unwrap().url_key, "high", "download = largest mesh (never the scad/img)");
        assert_eq!(viewer_mesh(&vs).unwrap().url_key, "low", "viewer = smallest 3mf");
    }
}
