/*
 * Syntax highlighting for fenced code blocks on content pages (highlight.js).
 *
 * Markdown renders a ```lang fence as <pre><code class="language-lang">…</code></pre>.
 * This highlights them in place, on load AND after an HTMX swap. The d2 diagram
 * source (<pre class="d2-source">) is deliberately EXCLUDED — it's diagram
 * source, not a programming language, and it's swapped out for the rendered SVG
 * anyway. highlight.min.js must load before this script.
 *
 * (highlightElement sets data-highlighted itself, so the :not() guard keeps a
 * re-run — e.g. after another swap — from double-processing.)
 */
(function () {
  "use strict";

  function highlightIn(root) {
    if (!window.hljs) return;
    (root || document)
      .querySelectorAll("pre:not(.d2-source) code:not([data-highlighted])")
      .forEach(function (el) {
        window.hljs.highlightElement(el);
      });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      highlightIn(document);
    });
  } else {
    highlightIn(document);
  }

  document.addEventListener("htmx:afterSwap", function (e) {
    highlightIn(e.target);
  });
})();
