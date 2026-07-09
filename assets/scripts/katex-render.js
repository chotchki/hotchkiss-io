/*
 * KaTeX typesetting for content pages.
 *
 * Math is authored as `$$…$$` (single `$` stays literal so prose prices don't
 * parse as math). web/markdown/transformer.rs emits each math node as a
 * source-carrying element — `<span class="math math-inline">TeX</span>` or
 * `<div class="math math-display">TeX</div>` — so a no-JS reader / crawler / LLM
 * sees the raw TeX. This typesets those elements in place, on load AND after an
 * HTMX swap. katex.min.js must load before this script.
 */
(function () {
  "use strict";

  function renderEl(el) {
    if (!window.katex) return;
    try {
      window.katex.render(el.textContent, el, {
        displayMode: el.classList.contains("math-display"),
        throwOnError: false,
      });
      el.setAttribute("data-rendered", "true");
    } catch {
      /* leave the TeX source visible on failure — never blank it out */
    }
  }

  function renderAll(root) {
    (root || document)
      .querySelectorAll(".math:not([data-rendered])")
      .forEach(renderEl);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      renderAll(document);
    });
  } else {
    renderAll(document);
  }

  // Re-typeset content swapped in by HTMX after the initial load.
  document.addEventListener("htmx:afterSwap", function (e) {
    renderAll(e.target);
  });
})();
