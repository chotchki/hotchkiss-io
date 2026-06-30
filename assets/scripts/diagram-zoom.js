/*
 * Diagram lightbox — click-to-zoom for inline D2 diagrams.
 *
 * Adapted from recon-gen's qs-lightbox.js (zero-dependency, no build step).
 * Our diagrams render as a data-URI <img> (vector SVG, so transform-zoom stays
 * crisp), emitted by web/markdown/diagram.rs as:
 *   <div class="diagram"><img data-zoomable="true" tabindex="0" ...></div>
 *
 * Diagrams arrive via an HTMX swap (GET /diagram/<hash>), so we bind with event
 * DELEGATION on the document rather than per-element — no re-init needed when a
 * diagram swaps in.
 *
 * Behaviour:
 *   - Click (or Enter/Space on a focused diagram) → fullscreen overlay with a
 *     clone of the diagram image.
 *   - Mouse wheel zooms about the cursor; +/- step by 1.25x; drag to pan.
 *   - Esc, ×, backdrop click, or a non-drag click closes; "Reset" re-fits.
 */
(function () {
  "use strict";

  let overlay = null;
  let viewport = null;
  let host = null;
  let state = null;

  const ZOOM_MIN = 0.2;
  const ZOOM_MAX = 20;
  const ZOOM_STEP = 1.25;

  function buildOverlay() {
    overlay = document.createElement("div");
    overlay.className = "diagram-lightbox";
    overlay.setAttribute("role", "dialog");
    overlay.setAttribute("aria-modal", "true");
    overlay.setAttribute("aria-label", "Diagram zoom view");
    overlay.hidden = true;

    const backdrop = document.createElement("div");
    backdrop.className = "diagram-lightbox__backdrop";

    viewport = document.createElement("div");
    viewport.className = "diagram-lightbox__viewport";

    host = document.createElement("div");
    host.className = "diagram-lightbox__host";
    viewport.appendChild(host);

    const controls = document.createElement("div");
    controls.className = "diagram-lightbox__controls";
    controls.appendChild(makeButton("−", "Zoom out", () => zoomBy(1 / ZOOM_STEP)));
    controls.appendChild(makeButton("Reset", "Reset zoom", fit));
    controls.appendChild(makeButton("+", "Zoom in", () => zoomBy(ZOOM_STEP)));
    controls.appendChild(makeButton("×", "Close", close));

    overlay.appendChild(backdrop);
    overlay.appendChild(viewport);
    overlay.appendChild(controls);

    backdrop.addEventListener("click", close);
    viewport.addEventListener("click", onViewportClick);
    viewport.addEventListener("wheel", onWheel, { passive: false });
    viewport.addEventListener("mousedown", onPanStart);
    document.addEventListener("keydown", onOverlayKey);
    window.addEventListener("resize", () => {
      if (overlay && !overlay.hidden) fit();
    });

    document.body.appendChild(overlay);
  }

  function makeButton(label, title, handler) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "diagram-lightbox__btn";
    b.textContent = label;
    b.title = title;
    b.setAttribute("aria-label", title);
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      handler();
    });
    return b;
  }

  function open(img) {
    if (overlay === null) buildOverlay();
    const clone = img.cloneNode(true);
    clone.removeAttribute("style");
    clone.removeAttribute("class");
    clone.removeAttribute("tabindex");
    // Drop srcset/sizes so the zoom view loads `src` — which the responsive
    // render sets to the LARGEST (original) variant (Phase CN). Otherwise the
    // browser would re-pick a small width-stepped variant off the inherited
    // `sizes` hint and the deep-zoom would be blurry.
    clone.removeAttribute("srcset");
    clone.removeAttribute("sizes");
    clone.className = "diagram-lightbox__img";

    host.replaceChildren(clone);
    state = { scale: 1, tx: 0, ty: 0, dragging: false, moved: false, startX: 0, startY: 0, lastX: 0, lastY: 0 };
    overlay.hidden = false;
    document.body.classList.add("diagram-lightbox-open");
    fit();
  }

  function close() {
    if (overlay === null) return;
    overlay.hidden = true;
    host.replaceChildren();
    document.body.classList.remove("diagram-lightbox-open");
  }

  function fit() {
    if (state === null) return;
    state.scale = 1;
    state.tx = 0;
    state.ty = 0;
    apply();
  }

  function apply() {
    host.style.transform =
      "translate(" + state.tx + "px, " + state.ty + "px) scale(" + state.scale + ")";
  }

  function clamp(next) {
    return Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, next));
  }

  function zoomBy(factor) {
    if (state === null) return;
    const rect = viewport.getBoundingClientRect();
    zoomAt(factor, rect.width / 2, rect.height / 2);
  }

  function zoomAt(factor, x, y) {
    const next = clamp(state.scale * factor);
    const real = next / state.scale;
    state.tx = x - (x - state.tx) * real;
    state.ty = y - (y - state.ty) * real;
    state.scale = next;
    apply();
  }

  function onWheel(e) {
    if (state === null) return;
    e.preventDefault();
    const rect = viewport.getBoundingClientRect();
    const factor = e.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP;
    zoomAt(factor, e.clientX - rect.left, e.clientY - rect.top);
  }

  function onPanStart(e) {
    if (state === null || e.button !== 0) return;
    if (e.target.closest(".diagram-lightbox__controls")) return;
    state.dragging = true;
    state.moved = false;
    state.startX = e.clientX;
    state.startY = e.clientY;
    state.lastX = e.clientX;
    state.lastY = e.clientY;
    viewport.classList.add("diagram-lightbox__viewport--grabbing");
    document.addEventListener("mousemove", onPanMove);
    document.addEventListener("mouseup", onPanEnd);
    e.preventDefault();
  }

  function onPanMove(e) {
    if (state === null || !state.dragging) return;
    state.tx += e.clientX - state.lastX;
    state.ty += e.clientY - state.lastY;
    state.lastX = e.clientX;
    state.lastY = e.clientY;
    if (Math.abs(e.clientX - state.startX) > 3 || Math.abs(e.clientY - state.startY) > 3) {
      state.moved = true;
    }
    apply();
  }

  function onPanEnd() {
    if (state === null) return;
    state.dragging = false;
    viewport.classList.remove("diagram-lightbox__viewport--grabbing");
    document.removeEventListener("mousemove", onPanMove);
    document.removeEventListener("mouseup", onPanEnd);
  }

  function onViewportClick(e) {
    if (state === null) return;
    if (state.moved) {
      state.moved = false;
      return;
    }
    if (e.target.closest(".diagram-lightbox__controls")) return;
    close();
  }

  function onOverlayKey(e) {
    if (overlay === null || overlay.hidden) return;
    if (e.key === "Escape") close();
    else if (e.key === "+" || e.key === "=") zoomBy(ZOOM_STEP);
    else if (e.key === "-" || e.key === "_") zoomBy(1 / ZOOM_STEP);
    else if (e.key === "0") fit();
  }

  // Delegated triggers — work for diagrams swapped in by HTMX after load AND for
  // content images (markdown `![]()`), which carry the same `data-zoomable` hook.
  function zoomableFrom(target) {
    const t = target.closest ? target.closest("img[data-zoomable]") : null;
    return t;
  }

  document.addEventListener("click", (e) => {
    const img = zoomableFrom(e.target);
    if (!img) return;
    if (window.getSelection && window.getSelection().toString()) return;
    e.preventDefault();
    open(img);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const img = zoomableFrom(document.activeElement);
    if (!img) return;
    e.preventDefault();
    open(img);
  });
})();
