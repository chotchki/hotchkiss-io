// EPUB reader (Phase DV) — mounts the vendored foliate-js engine on each
// `![](/media/<ref>)` epub embed. Fetches the gated `.epub` as a Blob (the
// `/media/file/<key>` byte route applies the min_role gate; the session cookie
// rides same-origin), hands it to a `<foliate-view>`, and restores the saved
// reading location. RTL is read from the book's OPF — `goLeft`/`goRight` are
// direction-aware, so manga turns the right way with no guessing. Degrades to the
// no-JS download link on any failure. Classic script (dynamic-imports the ES
// modules), mirrors audio-player.js's idempotent scan for HTMX-swapped embeds.
(function () {
  "use strict";

  const LOC_PREFIX = "epub-loc:"; // per-device resume (server sync deferred, like audio)
  // Keyboard routes to the most-recently-mounted reader. A volume page has exactly
  // one embed, so this is unambiguous in practice.
  let activeView = null;

  function ctrlButton(glyph, title, onClick) {
    const b = document.createElement("button");
    b.type = "button";
    b.title = title;
    b.setAttribute("aria-label", title);
    b.textContent = glyph;
    b.className =
      "px-3 py-1 rounded-full bg-navy/80 text-div-grey hover:bg-navy text-lg leading-none";
    b.addEventListener("click", onClick);
    return b;
  }

  async function mount(el) {
    if (el.dataset.mounted) return;
    el.dataset.mounted = "1";
    const src = el.dataset.src;
    const ref = el.dataset.ref;
    // "epub" (default) or "cbz" — picks the File name + type so foliate's makeBook
    // dispatches to the EPUB vs comic-book reader (Phase DW.8).
    const kind = el.dataset.kind === "cbz" ? "cbz" : "epub";
    const splash = el.querySelector(".epub-splash");
    const fail = (msg) => {
      if (splash) {
        splash.textContent = msg;
        splash.style.cursor = "pointer";
        splash.title = "Download the EPUB";
        splash.addEventListener("click", function () {
          window.location.href = src;
        });
      }
    };

    try {
      // The gated byte fetch — the min_role gate applies here (same-origin cookie).
      const resp = await fetch(src, { credentials: "same-origin" });
      if (!resp.ok) throw new Error("fetch " + src + " -> " + resp.status);
      const blob = await resp.blob();
      const fileName = kind === "cbz" ? "book.cbz" : "book.epub";
      const fileType =
        kind === "cbz"
          ? "application/vnd.comicbook+zip"
          : "application/epub+zip";
      const file = new File([blob], fileName, { type: fileType });

      // Dynamic-import foliate's view module — it registers <foliate-view> and
      // (via top-level await) its zip loader. Relative imports resolve under
      // /vendor/foliate/.
      await import("/vendor/foliate/view.js");
      const view = document.createElement("foliate-view");
      view.setAttribute("style", "width:100%;height:100%;display:block;");
      // Behind the splash (which is absolute inset-0 z-10) until we lift it.
      el.insertBefore(view, splash);
      await view.open(file);

      // Persist the CFI on every relocate; restore the saved one via init().
      view.addEventListener("relocate", function (e) {
        const cfi = e.detail && e.detail.cfi;
        if (cfi) {
          try {
            localStorage.setItem(LOC_PREFIX + ref, cfi);
          } catch (_) {}
        }
      });
      let saved = null;
      try {
        saved = localStorage.getItem(LOC_PREFIX + ref);
      } catch (_) {}
      try {
        await view.init({ lastLocation: saved || undefined });
      } catch (_) {
        // A stale/invalid saved CFI (the book changed) → start fresh, don't dead-end.
        await view.init({});
      }

      activeView = view;

      // Page-turn + fullscreen controls. goLeft/goRight are RTL-aware (foliate reads
      // page-progression-direction from the OPF), so these are correct for manga.
      const bar = document.createElement("div");
      bar.className =
        "epub-controls absolute bottom-2 left-1/2 -translate-x-1/2 z-20 flex items-center gap-2 bg-white/85 rounded-full px-2 py-1 shadow";
      bar.append(
        ctrlButton("‹", "Previous page", function () {
          view.goLeft();
        }),
        ctrlButton("⤢", "Fullscreen", function () {
          if (document.fullscreenElement) document.exitFullscreen();
          else if (el.requestFullscreen) el.requestFullscreen();
        }),
        ctrlButton("›", "Next page", function () {
          view.goRight();
        }),
      );
      el.appendChild(bar);

      if (splash) splash.remove();
    } catch (err) {
      fail("Couldn't load the book — tap to download it instead.");
      if (window.console) console.error("epub-reader:", err);
    }
  }

  // Arrow keys / vim h,l turn pages on the active reader (direction-aware).
  document.addEventListener("keydown", function (e) {
    if (!activeView) return;
    const k = e.key;
    if (k === "ArrowLeft" || k === "h") activeView.goLeft();
    else if (k === "ArrowRight" || k === "l") activeView.goRight();
  });

  function scan() {
    document.querySelectorAll(".epub-reader[data-src]").forEach(mount);
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", scan);
  } else {
    scan();
  }
  // The embed arrives via an HTMX swap (![](/media/<ref>) → /media/embed) — mount
  // it once it settles, like the audio player.
  document.addEventListener("htmx:afterSettle", scan);
})();
