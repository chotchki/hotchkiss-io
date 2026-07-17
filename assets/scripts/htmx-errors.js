"use strict";
(function () {
  // Phase DM.8: every admin mutation (hx-post/hx-delete) relies on an
  // hx-refresh / hx-redirect on success. A defense-in-depth reject — the 409
  // last-admin guard, the CZ 400 not-assignable guard, a 404 — fires
  // htmx:responseError with NO swap, so the click read as a silent no-op and
  // the admin assumed it worked. Surface the server's response text as a
  // dismissible banner so the failure is never invisible.
  //
  // Deferred + loaded after htmx (document order): it binds htmx events, so
  // htmx must be defined first.

  function ensure_toast() {
    var el = document.getElementById("app-error-toast");
    if (!el) {
      el = document.createElement("div");
      el.id = "app-error-toast";
      el.setAttribute("role", "alert");
      el.className =
        "fixed top-2 left-1/2 -translate-x-1/2 z-50 max-w-md " +
        "rounded-md border-2 border-navy bg-div-grey text-navy px-4 py-2 " +
        "shadow-lg cursor-pointer";
      el.addEventListener("click", function () {
        el.remove();
      });
      document.body.appendChild(el);
    }
    return el;
  }

  function show_toast(message) {
    var el = ensure_toast();
    el.textContent = message + "  (tap to dismiss)";
  }

  // 4xx/5xx WITHOUT a redirect. HX-Redirect responses (auth 401 → /login) are
  // handled by htmx itself and never reach here.
  document.body.addEventListener("htmx:responseError", function (evt) {
    var xhr = evt.detail && evt.detail.xhr;
    if (!xhr) {
      return;
    }
    // textContent (never innerHTML) — the body is server text, rendered as
    // text, so a stray error body can't inject markup. A short plain-text
    // reason ("A page with slug 'x' already exists") shows verbatim; a long
    // body (a full HTML 500 page) is replaced with a generic status line.
    var body = (xhr.responseText || "").trim();
    var message =
      body && body.length > 0 && body.length < 300
        ? body
        : "That action didn't go through (error " + xhr.status + ").";
    show_toast(message);
  });

  // A total network failure (server unreachable) fires htmx:sendError, not
  // responseError — there's no xhr status to read.
  document.body.addEventListener("htmx:sendError", function () {
    show_toast("Couldn't reach the server — check your connection and retry.");
  });
})();
