//! Inline diagram rendering for page markdown.
//!
//! Diagrams are authored INLINE in the markdown as a fenced ` ```d2 ` block. The
//! served page carries the **d2 source** (in a `<pre>`), so a crawler / LLM / a
//! no-JS reader sees the actual source — diffable, LLM-parsable. HTMX then GETs
//! `/diagram/<hash>` on load and swaps the source for the rendered SVG.
//!
//! Why source-in-HTML + HTMX swap (chris's design):
//!  - The served HTML's canonical content is the source, not an opaque base64
//!    blob — far friendlier to anything reading the page.
//!  - Progressive enhancement: no JS -> readable source; with JS -> diagram.
//!  - The render endpoint renders ONLY sources the server itself emitted
//!    (looked up by content hash in [`REGISTRY`]), so it is NOT an open
//!    "compile arbitrary d2" surface — no DoS / abuse vector.
//!
//! Hashing (chris's caution — a page may have many diagrams): the id is a
//! content hash of the **source bytes only** (SHA-256, 128-bit hex). Content-
//! addressed, so two different diagrams can't collide and two identical ones
//! dedupe harmlessly. Nothing page- or position-specific goes into the hash.
//!
//! Backend: the `d2` binary (`brew install d2`), shelled out lazily at the
//! endpoint (not at page render) and cached. A PATH resolver finds it even under
//! the mini's minimal LaunchAgent PATH. A missing d2 or bad source yields a
//! visible error block, never a 500.

use anyhow::anyhow;
use anyhow::Result;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use openssl::sha::sha256;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::sync::LazyLock;
use std::sync::Mutex;

/// Does a fenced code block's info-string name a diagram we render?
pub fn is_diagram_lang(lang: &str) -> bool {
    lang == "d2"
}

/// Is a usable `d2` binary present? (Test-only, so suites can branch on the
/// happy vs degraded path without hard-requiring d2.)
#[cfg(test)]
fn available() -> bool {
    D2_BIN.is_some()
}

/// hash -> d2 source, populated when a page renders. The render endpoint only
/// compiles sources that live here (i.e. ones the server itself emitted), so it
/// can't be driven to compile arbitrary client input.
static REGISTRY: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// hash -> rendered `<div><img></div>` fragment. In-memory, process lifetime
/// (rebuilt lazily). Mirrors the on-the-fly AVIF precedent.
static CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Resolved once: `$D2_BIN`, then the usual brew locations, then PATH. The
/// mini's LaunchAgent runs with a minimal PATH that excludes /opt/homebrew/bin,
/// so a bare "d2" can't be relied on there.
static D2_BIN: LazyLock<Option<String>> = LazyLock::new(resolve_d2_bin);

fn resolve_d2_bin() -> Option<String> {
    if let Ok(p) = std::env::var("D2_BIN")
        && !p.is_empty()
    {
        return Some(p);
    }
    for cand in ["/opt/homebrew/bin/d2", "/usr/local/bin/d2"] {
        if Path::new(cand).is_file() {
            return Some(cand.to_string());
        }
    }
    let on_path = Command::new("d2")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    on_path.then(|| "d2".to_string())
}

/// Content hash of the d2 source: SHA-256 truncated to 128 bits, hex. Content-
/// addressed (see module docs on the multi-diagram caution).
fn content_hash(source: &str) -> String {
    let digest = sha256(source.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Register a diagram's source (returns its content hash) so the endpoint can
/// render it later by hash. Called at page-render time.
pub fn register(source: &str) -> String {
    let hash = content_hash(source);
    REGISTRY
        .lock()
        .expect("diagram registry poisoned")
        .insert(hash.clone(), source.to_string());
    hash
}

/// The in-page placeholder: shows the d2 source (the LLM / no-JS payload) and
/// tells HTMX to swap it for the rendered SVG on load. Emitted on ONE line so it
/// survives the markdown AST round-trip (source newlines become `&#10;`, which a
/// `<pre>` still renders as line breaks).
pub fn placeholder(hash: &str, source: &str) -> String {
    let shown = html_escape(source).replace('\n', "&#10;");
    format!(
        "<div class=\"diagram my-4\" hx-get=\"/diagram/{hash}\" hx-trigger=\"load\" hx-swap=\"outerHTML\">\
<pre class=\"d2-source overflow-x-auto rounded-md bg-div-grey p-3 text-sm\"><code>{shown}</code></pre></div>"
    )
}

/// Render a previously-[`register`]ed diagram by hash, for the HTMX swap. Returns
/// `None` only when the hash isn't known (e.g. a stale tab after a restart);
/// a bad source or missing d2 returns `Some(error_block)` so HTMX still swaps in
/// something visible (never a 500).
pub fn render_registered(hash: &str) -> Option<String> {
    if let Some(hit) = CACHE.lock().expect("diagram cache poisoned").get(hash) {
        return Some(hit.clone());
    }
    let source = REGISTRY
        .lock()
        .expect("diagram registry poisoned")
        .get(hash)
        .cloned()?;

    let html = match render_d2(&source) {
        Ok(svg) => {
            let wrapped = wrap_svg(&svg);
            CACHE
                .lock()
                .expect("diagram cache poisoned")
                .insert(hash.to_string(), wrapped.clone());
            wrapped
        }
        // Don't cache errors — a fixed environment (d2 installed) recovers
        // without a restart.
        Err(e) => error_block("d2", &e.to_string()),
    };
    Some(html)
}

fn render_d2(source: &str) -> Result<String> {
    let bin = D2_BIN.as_deref().ok_or_else(|| {
        anyhow!("d2 not found — run `brew install d2` (looked at $D2_BIN, /opt/homebrew/bin, /usr/local/bin, PATH)")
    })?;

    let mut child = Command::new(bin)
        .arg("-") // read D2 from stdin
        .arg("-") // write SVG to stdout
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("failed to spawn d2 ({bin}): {e}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("d2 stdin unavailable"))?;
        stdin.write_all(source.as_bytes())?;
    } // drop stdin -> EOF so d2 starts compiling

    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("d2 failed: {}", err.trim()));
    }
    let svg = String::from_utf8_lossy(&out.stdout).into_owned();
    if svg.trim().is_empty() {
        return Err(anyhow!("d2 produced no output"));
    }
    Ok(svg)
}

/// Embed the SVG as a base64 `data:` URI inside a responsive `<img>` — isolated,
/// so its ids / fonts can't collide with the page or sibling diagrams. This is
/// the HTMX swap response, so it never touches the markdown round-trip.
/// In-flow diagrams are capped to a reasonable height so a tall diagram doesn't
/// dominate the page; click-to-zoom (`diagram-zoom.js`) shows the full thing.
const MAX_DIAGRAM_HEIGHT_PX: u32 = 480;

fn wrap_svg(svg: &str) -> String {
    // Give the SVG a definite intrinsic size. d2's outer <svg> has only a
    // viewBox, so an <img> can't size it and stretches it huge; with the size
    // injected, max-width:100% + max-height:<cap> scale it down proportionally.
    let sized = match natural_size(svg) {
        Some((w, h)) => with_intrinsic_size(svg, w, h),
        None => svg.to_string(),
    };
    let b64 = BASE64.encode(sized.as_bytes());
    // `data-zoomable` + tabindex/role are the click-to-zoom hook (diagram-zoom.js).
    format!(
        "<div class=\"diagram my-4\"><img class=\"mx-auto block cursor-zoom-in\" \
style=\"max-width:100%;max-height:{MAX_DIAGRAM_HEIGHT_PX}px\" alt=\"diagram\" \
data-zoomable=\"true\" tabindex=\"0\" role=\"button\" aria-label=\"Zoom diagram\" \
src=\"data:image/svg+xml;base64,{b64}\" /></div>"
    )
}

/// Natural (width, height) in px from the SVG's first `viewBox`
/// ("minx miny width height").
fn natural_size(svg: &str) -> Option<(u32, u32)> {
    let start = svg.find("viewBox=\"")? + "viewBox=\"".len();
    let end = svg[start..].find('"')? + start;
    let mut nums = svg[start..end].split_whitespace().skip(2);
    let w = nums.next()?.parse::<f64>().ok()?;
    let h = nums.next()?.parse::<f64>().ok()?;
    Some((w.ceil() as u32, h.ceil() as u32))
}

/// Inject `width`/`height` into the (first) `<svg>` so an `<img>` can size it.
fn with_intrinsic_size(svg: &str, w: u32, h: u32) -> String {
    svg.replacen("<svg", &format!("<svg width=\"{w}\" height=\"{h}\""), 1)
}

/// A visible, non-fatal error block for a broken/missing diagram. Surfaces the
/// failure (chris's integrity rule: never silently swallow it) without taking
/// the page down. Labelled in text, so it doesn't rely on color alone.
pub fn error_block(lang: &str, message: &str) -> String {
    format!(
        "<div class=\"diagram-error my-4 rounded-md border-2 border-red-700 bg-red-50 p-3 text-sm text-red-900\">\
<strong>Diagram error</strong> (<code>{}</code>): {}</div>",
        html_escape(lang),
        html_escape(message)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_carries_source_and_swap() {
        let src = "x -> y\ny -> z";
        let hash = register(src);
        let ph = placeholder(&hash, src);
        assert!(ph.contains(&format!("hx-get=\"/diagram/{hash}\"")), "needs the swap target");
        assert!(ph.contains("hx-trigger=\"load\""));
        assert!(ph.contains("x -&gt; y"), "source must be in the HTML (escaped): {ph}");
        assert!(ph.contains("&#10;"), "source newlines kept for <pre> display, round-trip-safe");
        assert!(!ph.contains('\n'), "placeholder must be one line for the md round-trip");
    }

    #[test]
    fn content_hash_is_stable_and_distinct() {
        // identical source -> identical hash (dedupes harmlessly on a page)
        assert_eq!(content_hash("a -> b"), content_hash("a -> b"));
        // different source -> different hash (no collision across diagrams)
        assert_ne!(content_hash("a -> b"), content_hash("a -> c"));
        assert_eq!(content_hash("a -> b").len(), 32, "128-bit hex");
    }

    #[test]
    fn unknown_hash_is_none() {
        assert!(render_registered("deadbeefdeadbeefdeadbeefdeadbeef").is_none());
    }

    #[test]
    fn natural_size_from_viewbox() {
        assert_eq!(
            natural_size(r#"<svg viewBox="0 0 256 600"><g></g></svg>"#),
            Some((256, 600))
        );
        assert_eq!(natural_size("<svg></svg>"), None);
    }

    #[test]
    fn registered_d2_renders_or_errors_visibly() {
        let hash = register("x -> y -> z");
        let out = render_registered(&hash).expect("registered hash should resolve");
        if available() {
            assert!(out.contains("data:image/svg+xml;base64,"), "expected the diagram image: {out}");
            assert!(out.contains("max-height:"), "in-flow diagram must cap its height: {out}");
            assert!(out.contains("data-zoomable=\"true\""), "click-to-zoom hook missing: {out}");
        } else {
            assert!(out.contains("diagram-error"), "expected a visible error block: {out}");
        }
    }

    #[test]
    fn broken_registered_d2_is_error_block_not_panic() {
        if !available() {
            return;
        }
        let hash = register("x -> -> -> {{{");
        let out = render_registered(&hash).expect("hash resolves");
        assert!(out.contains("diagram-error"), "broken d2 should be a visible error: {out}");
    }

    #[test]
    fn error_block_escapes_and_labels() {
        let b = error_block("d2", "bad <thing> & stuff");
        assert!(b.contains("Diagram error"));
        assert!(b.contains("&lt;thing&gt;"), "must escape HTML in the message");
        assert!(!b.contains("<thing>"), "raw HTML must not leak through");
    }

    #[test]
    fn is_diagram_lang_matches() {
        assert!(is_diagram_lang("d2"));
        assert!(!is_diagram_lang("dot"));
        assert!(!is_diagram_lang("rust"));
    }
}
