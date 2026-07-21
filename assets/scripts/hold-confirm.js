// ED.7 — hold-to-confirm, the site-wide replacement for EVERY confirm() and
// hx-confirm dialog (no modals — chris's rule). Any [data-hold-confirm]
// element requires a ~800ms press-and-hold: a fill sweeps across the button,
// releasing early cancels, completing the hold lets the NEXT click through a
// capture-phase gate (which otherwise blocks the click before htmx or any
// other listener sees it). Keyboard clicks (detail === 0) pass ungated —
// a keyboard can't hold, and locking keyboard users out entirely is worse
// than skipping the ceremony on an admin-only site.
(function () {
  const HOLD_MS = 800;

  // Capture-phase gate: a pointer click on a hold-confirm element only passes
  // once a completed hold armed it. stopPropagation keeps the event from ever
  // reaching htmx's / the page's own listeners.
  document.addEventListener(
    "click",
    (e) => {
      const el = e.target.closest && e.target.closest("[data-hold-confirm]");
      if (!el) return;
      if (e.detail === 0) return; // keyboard activation — see header comment
      if (el.dataset.holdOk) return;
      e.preventDefault();
      e.stopPropagation();
    },
    true,
  );

  function fillOf(el) {
    let fill = el.querySelector(":scope > .hold-confirm-fill");
    if (!fill) {
      if (getComputedStyle(el).position === "static") {
        el.style.position = "relative";
      }
      el.style.touchAction = "none";
      fill = document.createElement("span");
      fill.className = "hold-confirm-fill";
      fill.style.cssText =
        "position:absolute;inset:0;background:rgba(255,255,255,0.35);" +
        "transform:scaleX(0);transform-origin:left;pointer-events:none;";
      el.appendChild(fill);
    }
    return fill;
  }

  let holding = null; // { el, fill, timer }

  function cancelHold(reset) {
    if (!holding) return;
    clearTimeout(holding.timer);
    if (reset) {
      holding.fill.style.transition = "transform 150ms ease-out";
      holding.fill.style.transform = "scaleX(0)";
    }
    holding = null;
  }

  document.addEventListener("pointerdown", (e) => {
    const el = e.target.closest && e.target.closest("[data-hold-confirm]");
    if (!el) return;
    const fill = fillOf(el);
    fill.style.transition = "transform " + HOLD_MS + "ms linear";
    // Force a reflow so the transition starts from 0 even after a reset.
    void fill.offsetWidth;
    fill.style.transform = "scaleX(1)";
    holding = {
      el,
      fill,
      timer: setTimeout(() => {
        el.dataset.holdOk = "1";
        // Disarm shortly after release — long enough for the natural click
        // that follows pointerup (and the e2e's synthetic one) to pass.
        const disarm = () => {
          delete el.dataset.holdOk;
          fill.style.transition = "transform 150ms ease-out";
          fill.style.transform = "scaleX(0)";
        };
        el.addEventListener("pointerup", () => setTimeout(disarm, 200), {
          once: true,
        });
      }, HOLD_MS),
    };
  });

  for (const evt of ["pointerup", "pointercancel"]) {
    document.addEventListener(evt, () => {
      // Completed holds keep their armed state (disarm handles cleanup);
      // an early release resets the fill and never arms.
      if (holding && !holding.el.dataset.holdOk) {
        cancelHold(true);
      } else {
        cancelHold(false);
      }
    });
  }

  // A long-press on touch pops the context menu — suppress it on hold targets.
  document.addEventListener("contextmenu", (e) => {
    if (e.target.closest && e.target.closest("[data-hold-confirm]")) {
      e.preventDefault();
    }
  });
})();
